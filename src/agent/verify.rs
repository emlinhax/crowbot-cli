//! Auto-verify grounding: when a top-level run that changed files would end, run the project's
//! configured check (the `verify` setting; unset = off) and, on a clean failure, hand the output
//! back so the model fixes it instead of ending broken. `verify` is not an untrusted-settable key
//! (trust.toml), so only the user's own or a trusted folder's config sets the command crowbot runs.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::agent::model_text;
use crate::config::limits;
use crate::config::paths::Paths;
use crate::config::settings::Settings;
use crate::conversation::message::Part;
use crate::io::proc::{self, Ended, ShellCommand};
use crate::text::template::fill;
use crate::text::truncate::{self, Keep};
use crate::tools::shell;

/// The follow-up to hand back when the configured check fails, or `None` to let the run end: no
/// command set, nothing changed, out of attempts, not a top-level run, no shell, or the check
/// passed. A timeout, signal or cancel is inconclusive, not a failure, so it never loops the run.
pub async fn failure(
    settings: &Settings,
    paths: &Paths,
    cancel: &CancellationToken,
    changed: bool,
    top_level: bool,
    attempts: u32,
) -> Option<Vec<Part>> {
    let command = settings
        .verify
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())?;
    let agent = &limits::get().agent;
    if !top_level || !changed || attempts >= agent.verify_attempts.value {
        return None;
    }
    let shell = shell::resolve(settings.shell.as_deref());
    let program = shell.program.as_deref()?;
    let tools = &limits::get().tools;
    let mut out = Vec::new();
    let ended = proc::run(
        &ShellCommand {
            program,
            args: &shell.args,
            command,
            cwd: &paths.project,
            env: &shell.env,
            unset: &shell.unset,
            timeout: Duration::from_secs(agent.verify_timeout_secs.value),
            drain: tools.bash_drain_ms.ms(),
        },
        cancel,
        &mut |chunk| out.extend_from_slice(chunk),
    )
    .await
    .ok()?;
    if !matches!(ended, Ended::Exited(Some(code)) if code != 0) {
        return None;
    }
    let text = String::from_utf8_lossy(&out);
    let cut = truncate::cut(
        &text,
        Keep::Tail,
        tools.max_lines.value,
        tools.max_bytes.value,
    );
    let message = fill(
        &model_text::get().verify_failed,
        &[("command", command), ("output", cut.text.trim())],
    );
    Some(vec![Part::Reminder { text: message }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::paths::Paths;
    use crate::config::settings::{self, Overrides};

    fn setup() -> (tempfile::TempDir, Paths, Settings) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("proj"));
        crate::io::fs::make_dir(&paths.home).unwrap();
        crate::io::fs::make_dir(&paths.project).unwrap();
        let settings = settings::load(&paths, &Overrides::default()).unwrap();
        (dir, paths, settings)
    }

    #[tokio::test]
    async fn the_gate_skips_before_the_check_would_run() {
        let (_dir, paths, mut settings) = setup();
        let cancel = CancellationToken::new();
        // No command: nothing to run.
        assert!(
            failure(&settings, &paths, &cancel, true, true, 0)
                .await
                .is_none()
        );
        // With one set, the gate still skips a subagent, an unchanged run, and the attempt cap —
        // each returns before the check would run, so this holds on every platform.
        settings.verify = Some("exit 1".into());
        let cap = limits::get().agent.verify_attempts.value;
        assert!(
            failure(&settings, &paths, &cancel, true, false, 0)
                .await
                .is_none(),
            "subagent"
        );
        assert!(
            failure(&settings, &paths, &cancel, false, true, 0)
                .await
                .is_none(),
            "unchanged"
        );
        assert!(
            failure(&settings, &paths, &cancel, true, true, cap)
                .await
                .is_none(),
            "out of attempts"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_check_is_handed_back_a_passing_one_lets_it_end() {
        let (_dir, paths, mut settings) = setup();
        let cancel = CancellationToken::new();
        settings.verify = Some("true".into());
        assert!(
            failure(&settings, &paths, &cancel, true, true, 0)
                .await
                .is_none(),
            "a passing check lets the run end"
        );
        settings.verify = Some("echo boom-from-check; exit 1".into());
        let parts = failure(&settings, &paths, &cancel, true, true, 0)
            .await
            .expect("a failing check is handed back");
        let Part::Reminder { text } = &parts[0] else {
            panic!("a reminder");
        };
        assert!(text.contains("boom-from-check"), "{text}");
        assert!(text.contains("echo boom-from-check"), "{text}");
    }
}
