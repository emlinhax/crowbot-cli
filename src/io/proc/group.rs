//! A process and everything it starts, kept together so they end together: a Job Object on
//! Windows, a process group on Unix. Shared by commands run to their end (tree.rs) and programs
//! that keep running (child.rs).

use std::io;

/// Starts `command` in a group of its own, with no console window on Windows, so our own Ctrl+C
/// never reaches it and the group can be ended as one.
pub fn contain(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    // CEILING: a child that calls setsid or runs under `set -m` leaves the group and survives;
    // closing that needs PR_SET_CHILD_SUBREAPER or a cgroup.
    #[cfg(unix)]
    command.process_group(0);
}

/// Every process a command started. Dropping it kills them all.
pub struct Tree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    group: i32,
}

// SAFETY: the job handle is only used through thread-safe Win32 calls.
#[cfg(windows)]
unsafe impl Send for Tree {}

impl Tree {
    #[cfg(windows)]
    pub fn adopt(child: &tokio::process::Child) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: plain Win32 calls on a handle we own; the struct is zeroed as the API expects.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            // From here every early return closes the job, and the caller's `?` drops the
            // child, which `kill_on_drop` ends: nothing runs untracked.
            let tree = Self { job };
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            // CEILING: a grandchild spawned before this line escapes the job; closing that gap
            // needs CREATE_SUSPENDED and resuming the main thread after assignment.
            let handle = child.raw_handle().ok_or_else(untracked)?;
            if AssignProcessToJobObject(job, handle) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(tree)
        }
    }

    #[cfg(unix)]
    pub fn adopt(child: &tokio::process::Child) -> io::Result<Self> {
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .ok_or_else(untracked)?;
        Ok(Self { group })
    }

    pub fn kill(&self) {
        #[cfg(windows)]
        // SAFETY: the job handle stays valid until drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
        #[cfg(unix)]
        // SAFETY: signalling a process group we created; failure means it is already gone.
        unsafe {
            libc::killpg(self.group, libc::SIGKILL);
        }
    }
}

fn untracked() -> io::Error {
    io::Error::other("the command exited before it could be tracked")
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.kill();
        #[cfg(windows)]
        // SAFETY: closing the handle we created, once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}
