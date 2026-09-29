use crate::api::Api;
use crate::io::fetch::Fetch;
use crate::io::http::Http;
use crate::paths::Paths;
use crate::settings::{self, Overrides, Settings};
use crate::{auth, limits};

/// What every command and the agent run against, built once at startup.
pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    /// crowbot's endpoints.
    pub api: Api,
    /// Everything else on the web (the webfetch tool).
    pub fetch: Fetch,
}

impl App {
    pub fn load(flags: &Overrides) -> anyhow::Result<Self> {
        let paths = Paths::resolve()?;
        let settings = settings::load(&paths, flags)?;
        let http = Http::new(limits::get().http.connect_timeout_ms.ms())?;
        let key = auth::load(&paths)?.map(|k| k.secret);
        Ok(Self {
            api: Api::new(http, key),
            fetch: Fetch::new(limits::get().tools.webfetch_timeout_secs.secs())?,
            paths,
            settings,
        })
    }

    /// Defaults only, rooted in `project`, with no key: for tests that never reach the network.
    #[cfg(test)]
    pub fn for_tests(project: &std::path::Path) -> Self {
        let paths = Paths::at(project.join(".crowbot-home"), project.to_path_buf());
        let settings = settings::load(&paths, &Overrides::default()).expect("defaults load");
        let http = Http::new(limits::get().http.connect_timeout_ms.ms()).expect("http client");
        Self {
            api: Api::new(http, None),
            fetch: Fetch::new(limits::get().tools.webfetch_timeout_secs.secs())
                .expect("fetch client"),
            paths,
            settings,
        }
    }
}
