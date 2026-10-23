use crate::api::Api;
use crate::auth;
use crate::config::limits;
use crate::config::paths::Paths;
use crate::config::settings::{self, Overrides, Settings};
use crate::io::fetch::Fetch;
use crate::io::http::Http;

/// What every command and the agent run against, built once at startup.
pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    /// What the command line set, which beats what a resumed session recorded.
    pub flags: Overrides,
    /// The transport crowbot's API and the release listing share.
    pub http: Http,
    /// crowbot's endpoints.
    pub api: Api,
    /// Everything else on the web: webfetch and forums.
    pub fetch: Fetch,
    /// What was wrong with the user's agent files, or left out of them, to say once.
    pub agent_notes: Vec<String>,
}

impl App {
    pub fn load(flags: &Overrides) -> anyhow::Result<Self> {
        let paths = Paths::resolve()?;
        let agent_notes = crate::agent::kinds::init(&paths);
        let settings = settings::load(&paths, flags)?;
        // An unreadable auth.json leaves the run keyless, so `logout` and `login --status` still
        // work.
        let key = auth::load(&paths).ok().flatten().map(|k| k.secret);
        let mut app = Self::assemble(paths, settings, key)?;
        app.flags = flags.clone();
        app.agent_notes = agent_notes;
        Ok(app)
    }

    /// Defaults only, rooted in `project`, with no key: for tests that never reach the network.
    #[cfg(test)]
    pub fn for_tests(project: &std::path::Path) -> Self {
        let paths = Paths::at(project.join(".crowbot-home"), project.to_path_buf());
        let settings = settings::load(&paths, &Overrides::default()).expect("defaults load");
        Self::assemble(paths, settings, None).expect("clients")
    }

    /// The clients, built once: crowbot's API shares the transport the app keeps.
    fn assemble(paths: Paths, settings: Settings, key: Option<String>) -> anyhow::Result<Self> {
        let limits = &limits::get().http;
        let http = Http::new(limits.connect_timeout_ms.ms())?;
        Ok(Self {
            api: Api::new(http.clone(), key),
            http,
            fetch: Fetch::new(limits.fetch_timeout_secs.secs())?,
            paths,
            settings,
            flags: Overrides::default(),
            agent_notes: Vec::new(),
        })
    }
}
