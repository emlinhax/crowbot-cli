use crate::api::Api;
use crate::io::http::Http;
use crate::limits;
use crate::paths::Paths;
use crate::settings::{self, Overrides, Settings};

/// What every command and the agent run against, built once at startup.
pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    pub api: Api,
}

impl App {
    pub fn load(flags: &Overrides) -> anyhow::Result<Self> {
        let paths = Paths::resolve()?;
        let settings = settings::load(&paths, flags)?;
        let http = Http::new(limits::get().http.connect_timeout_ms.ms())?;
        Ok(Self {
            paths,
            settings,
            api: Api::new(http),
        })
    }
}
