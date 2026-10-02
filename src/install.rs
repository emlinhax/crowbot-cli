//! crowbot installed for this user: the program in a folder of its own, that folder on PATH, and
//! on Windows an Apps & Features entry whose uninstall runs `crowbot uninstall`. `~/.crowbot`
//! (keys, sessions) is never touched.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::io::{self, fs::Access, user_path};
use crate::paths::Paths;
use crate::release;
use crate::text::template::fill;

static DATA: LazyLock<Data> = LazyLock::new(|| {
    toml::from_str(include_str!("../data/install.toml"))
        .expect("data/install.toml is checked by tests")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    folder: Folder,
    entry: Entry,
    text: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Folder {
    windows: String,
    unix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: String,
    name: String,
    publisher: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    offer: String,
    installed: String,
    added: String,
    already: String,
    profile: String,
    uninstalled: String,
    missing: String,
}

/// Where the installed program lives.
pub struct Place {
    pub dir: PathBuf,
    pub exe: PathBuf,
}

impl Place {
    /// This user's install folder on this platform.
    pub fn here() -> anyhow::Result<Self> {
        let home = dirs::home_dir().context("no home directory")?;
        let local = dirs::data_local_dir().unwrap_or_else(|| home.clone());
        let (folder, name) = if cfg!(windows) {
            (&DATA.folder.windows, "crowbot.exe")
        } else {
            (&DATA.folder.unix, "crowbot")
        };
        let dir = fill(
            folder,
            &[
                ("local", &local.to_string_lossy()),
                ("home", &home.to_string_lossy()),
            ],
        );
        let dir = PathBuf::from(dir);
        Ok(Self {
            exe: dir.join(name),
            dir,
        })
    }
}

/// Copies `exe` into `place` (unless it already runs from there), puts the folder on PATH and
/// lists it in Apps & Features.
pub fn install(place: &Place, exe: &Path) -> anyhow::Result<String> {
    if !io::fs::same_file(exe, &place.exe) {
        io::fs::make_dir(&place.dir)?;
        let staged = place.dir.join(".crowbot.new");
        io::fs::write_executable(&staged, &io::fs::read_bytes(exe)?)?;
        io::fs::replace_executable(&staged, &place.exe)?;
    }
    let change = user_path::add(&place.dir)?;
    let shown = place.dir.display().to_string();
    let exe_shown = place.exe.display().to_string();
    let uninstall = format!("\"{exe_shown}\" uninstall");
    let version = release::tag().unwrap_or(env!("CARGO_PKG_VERSION"));
    let entry = &DATA.entry;
    user_path::register_app(
        &entry.key,
        &[
            ("DisplayName", &entry.name),
            ("Publisher", &entry.publisher),
            ("DisplayVersion", version),
            ("InstallLocation", &shown),
            ("DisplayIcon", &exe_shown),
            ("UninstallString", &uninstall),
        ],
        &[("NoModify", 1), ("NoRepair", 1)],
    )?;
    let text = &DATA.text;
    let next = match change {
        user_path::Change::Added => text.added.clone(),
        user_path::Change::Already => text.already.clone(),
        user_path::Change::ShellProfile => fill(&text.profile, &[("dir", &shown)]),
    };
    Ok(format!(
        "{}{next}",
        fill(&text.installed, &[("dir", &shown)])
    ))
}

/// Takes `place` off PATH and out of Apps & Features, and removes the program. The program
/// cannot delete the folder it runs from on Windows, so that waits until it has exited.
pub fn uninstall(place: &Place, exe: &Path, paths: &Paths) -> anyhow::Result<String> {
    let text = &DATA.text;
    let shown = place.dir.display().to_string();
    if !place.exe.exists() {
        return Ok(fill(&text.missing, &[("dir", &shown)]));
    }
    user_path::remove(&place.dir)?;
    user_path::unregister_app(&DATA.entry.key)?;
    remove_program(place, exe)?;
    Ok(fill(
        &text.uninstalled,
        &[("dir", &shown), ("home", &paths.home.display().to_string())],
    ))
}

#[cfg(windows)]
fn remove_program(place: &Place, exe: &Path) -> anyhow::Result<()> {
    // The folder is crowbot's own on Windows, so all of it goes.
    if exe.starts_with(&place.dir) {
        io::proc::after_exit(&format!("rmdir /s /q \"{}\"", place.dir.display()))?;
        return Ok(());
    }
    io::fs::remove_dir(&place.dir)?;
    Ok(())
}

#[cfg(not(windows))]
fn remove_program(place: &Place, _exe: &Path) -> anyhow::Result<()> {
    // ~/.local/bin holds other programs, so only crowbot goes; a running one keeps its image.
    io::fs::remove(&place.exe)?;
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// The user said no to installing when asked; they are not asked again.
    #[serde(default)]
    declined: bool,
}

/// At an interactive start on Windows, a released crowbot run from anywhere but its install
/// folder (a double-clicked download) offers to install itself, once. A development build never
/// asks, so test sessions are never held up by it.
pub fn offer(paths: &Paths) -> anyhow::Result<()> {
    if !cfg!(windows) || release::tag().is_none() {
        return Ok(());
    }
    let place = Place::here()?;
    let state_file = paths.home.join("install.json");
    let state: State = io::fs::read_string(&state_file)?
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if place.exe.exists() || state.declined {
        return Ok(());
    }
    let exe = io::proc::current_exe()?;
    let shown = place.dir.display().to_string();
    io::term::out(&fill(&DATA.text.offer, &[("dir", &shown)]));
    let answer = io::term::read_line()?.unwrap_or_default();
    if matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes") {
        io::term::out(&format!("{}\n", install(&place, &exe)?));
    } else {
        let declined = serde_json::to_vec(&State { declined: true })?;
        io::fs::write_atomic(&state_file, &declined, Access::Shared)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_folder_is_the_users_own() {
        let place = Place::here().unwrap();
        let shown = place.dir.to_string_lossy().replace('\\', "/");
        if cfg!(windows) {
            assert!(shown.ends_with("/Programs/crowbot"), "{shown}");
            assert!(place.exe.ends_with("crowbot.exe"));
        } else {
            assert!(shown.ends_with("/.local/bin"), "{shown}");
            assert!(place.exe.ends_with("crowbot"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn install_copies_the_program_and_uninstall_takes_it_away() {
        let root = tempfile::tempdir().unwrap();
        let exe = root
            .path()
            .join("Downloads/crowbot-x86_64-unknown-linux-musl");
        io::fs::make_dir(exe.parent().unwrap()).unwrap();
        io::fs::write_executable(&exe, b"the program").unwrap();
        let place = Place {
            dir: root.path().join("bin"),
            exe: root.path().join("bin/crowbot"),
        };
        let said = install(&place, &exe).unwrap();
        assert_eq!(io::fs::read_bytes(&place.exe).unwrap(), b"the program");
        assert!(said.contains("is not on your PATH yet"), "{said}");
        // Installing from the installed copy changes nothing.
        install(&place, &place.exe).unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let said = uninstall(&place, &exe, &paths).unwrap();
        assert!(said.starts_with("Uninstalled from"), "{said}");
        assert!(!place.exe.exists());
        assert!(
            uninstall(&place, &exe, &paths)
                .unwrap()
                .contains("is not installed")
        );
    }
}
