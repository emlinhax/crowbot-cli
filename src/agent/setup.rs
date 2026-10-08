//! What a session needs before its first run, set up the same way for both frontends. It is per
//! session, not part of `App`: `logout` should not load the model catalog.

use anyhow::anyhow;

use crate::api::models::Model;
use crate::app::App;
use crate::live::state::Shared;
use crate::mode;
use crate::models::{self, Catalog};
use crate::session::transcript::Transcript;
use crate::tools::Registry;

pub struct Setup {
    pub catalog: Catalog,
    pub model: Model,
    pub tools: Registry,
    pub shared: Shared,
    pub transcript: Transcript,
}

impl Setup {
    pub async fn load(app: &App) -> anyhow::Result<Self> {
        let catalog = models::load(&app.api, &app.paths, false).await;
        // crowbot silently swaps an unknown model id for its default, so refuse it here instead.
        let model = catalog.get(&app.settings.model).cloned().ok_or_else(|| {
            anyhow!(
                "unknown model `{}`; see `crowbot models`",
                app.settings.model
            )
        })?;
        let mode = mode::get(&app.settings.mode).ok_or_else(|| anyhow!("mode was validated"))?;
        Ok(Self {
            catalog,
            model,
            tools: Registry::builtin(app.settings.shell.as_deref()),
            shared: Shared::new(mode),
            transcript: Transcript::create(&app.paths),
        })
    }
}
