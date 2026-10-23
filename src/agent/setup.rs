//! What a session needs before its first run, set up the same way for both frontends. It is per
//! session, not part of `App`: `logout` should not load the model catalog.

use std::path::Path;

use anyhow::anyhow;

use crate::api::models::Model;
use crate::app::App;
use crate::config::settings::{Overrides, Settings};
use crate::live::state::Shared;
use crate::mcp::Servers;
use crate::mode::{self, Mode};
use crate::models::{self, Catalog};
use crate::session::entry::State;
use crate::session::transcript::{Resumed, Transcript};
use crate::snapshot::Shadow;
use crate::tools::Registry;

pub struct Setup {
    pub catalog: Catalog,
    pub model: Model,
    /// The effort to ask for: the command line's, else the resumed session's, else the setting.
    pub effort: Option<String>,
    pub tools: Registry,
    pub shared: Shared,
    pub transcript: Transcript,
    /// Set when the session goes on from a file.
    pub resumed: Option<Resumed>,
    /// The model the resumed session used, when crowbot no longer serves it.
    pub gone: Option<String>,
    /// The project's files before each prompt changed them, for /undo.
    pub shadow: Shadow,
    /// The MCP servers config.toml names, started.
    pub mcp: Servers,
}

/// A session that goes on from a file.
pub struct Continue<'a> {
    pub path: &'a Path,
    /// Whether its recorded mode comes back too. A headless run keeps the mode it was given, so a
    /// script never inherits AUTO.
    pub mode: bool,
}

/// What a session goes on with.
pub struct Going {
    pub model: Model,
    pub effort: Option<String>,
    pub mode: &'static Mode,
    /// The model the session recorded, when crowbot no longer serves it.
    pub gone: Option<String>,
}

impl Setup {
    pub async fn load(app: &App, from: Option<Continue<'_>>) -> anyhow::Result<Self> {
        // A role file that will not parse fails the start, naming it, like a config file: a role
        // missing without a word would leave the agent unable to dispatch it.
        crate::agent::roles::load(&app.paths)?;
        // CEILING: a slow server holds the first screen for up to `mcp.start_secs`. Upgrade:
        // start servers behind the welcome screen and hold only the first request for them.
        let (catalog, shadow, mut mcp) = tokio::join!(
            models::load(&app.api, &app.paths, false),
            Shadow::open(&app.paths),
            Servers::start(&app.settings, &app.paths, &app.http),
        );
        let (transcript, resumed) = match &from {
            Some(from) => {
                let (transcript, resumed) = Transcript::open(from.path)?;
                (transcript, Some(resumed))
            }
            None => (Transcript::create(&app.paths), None),
        };
        let recorded = resumed.as_ref().and_then(|r| r.state.as_ref());
        let mode_too = from.as_ref().is_some_and(|f| f.mode);
        let going = going_on(app, &catalog, recorded, mode_too)?;
        let mut tools = Registry::builtin(app.settings.shell.as_deref());
        crate::tools::mcp::join(&mut mcp, &mut tools);
        Ok(Self {
            catalog,
            model: going.model,
            effort: going.effort,
            tools,
            shared: Shared::new(going.mode),
            transcript,
            resumed,
            gone: going.gone,
            shadow,
            mcp,
        })
    }
}

/// The model, effort and mode to go on with, from the command line, then what the session
/// `recorded`, then the settings; the recorded mode only when `mode_too`.
pub fn going_on(
    app: &App,
    catalog: &Catalog,
    recorded: Option<&State>,
    mode_too: bool,
) -> anyhow::Result<Going> {
    let mut wanted = chosen(&app.settings, &app.flags, recorded, mode_too);
    // A recorded model crowbot stopped serving gives way to the setting; a typed one is an error
    // below.
    let gone = (recorded.is_some() && catalog.get(&wanted.model).is_none())
        .then(|| std::mem::replace(&mut wanted.model, app.settings.model.clone()))
        .filter(|_| app.flags.model.is_none());
    // crowbot silently swaps an unknown model id for its default, so refuse it here instead.
    let model = catalog
        .get(&wanted.model)
        .cloned()
        .ok_or_else(|| anyhow!("unknown model `{}`; see `crowbot models`", wanted.model))?;
    let mode_id = wanted.mode.as_deref().unwrap_or(&app.settings.mode);
    let mode = mode::get(mode_id)
        .or_else(|| mode::get(&app.settings.mode))
        .ok_or_else(|| anyhow!("mode was validated"))?;
    Ok(Going {
        model,
        effort: wanted.effort,
        mode,
        gone,
    })
}

/// What to go on with: the command line's choice, else what the session recorded, else the
/// settings. The mode comes back only when `mode_too`.
fn chosen(
    settings: &Settings,
    flags: &Overrides,
    recorded: Option<&State>,
    mode_too: bool,
) -> State {
    let Some(recorded) = recorded else {
        return State {
            model: settings.model.clone(),
            effort: settings.effort.clone(),
            mode: Some(settings.mode.clone()),
        };
    };
    let recorded_mode = recorded.mode.clone().filter(|_| mode_too);
    State {
        model: match &flags.model {
            Some(_) => settings.model.clone(),
            None => recorded.model.clone(),
        },
        effort: match &flags.effort {
            Some(_) => settings.effort.clone(),
            None => recorded.effort.clone(),
        },
        mode: match &flags.mode {
            Some(_) => Some(settings.mode.clone()),
            None => recorded_mode.or_else(|| Some(settings.mode.clone())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        let dir = tempfile::tempdir().unwrap();
        App::for_tests(dir.path()).settings
    }

    #[test]
    fn the_command_line_beats_the_recorded_state_which_beats_the_settings() {
        let settings = settings();
        let recorded = State {
            model: "fake-coder".into(),
            effort: Some("high".into()),
            mode: Some("auto".into()),
        };
        let none = Overrides::default();
        let resumed = chosen(&settings, &none, Some(&recorded), true);
        assert_eq!(resumed, recorded);
        // Headless keeps its own mode.
        let headless = chosen(&settings, &none, Some(&recorded), false);
        assert_eq!(headless.mode, Some(settings.mode.clone()));
        let flagged = Overrides {
            model: Some(settings.model.clone()),
            ..Overrides::default()
        };
        let mixed = chosen(&settings, &flagged, Some(&recorded), true);
        assert_eq!(
            (mixed.model.as_str(), mixed.effort.as_deref()),
            (settings.model.as_str(), Some("high"))
        );
        let fresh = chosen(&settings, &none, None, true);
        assert_eq!(fresh.model, settings.model);
    }
}
