//! Every tests/fixtures/scenarios/*.toml, run by one runner: a prompt, a mode, scripted model
//! replies, and what must be true afterwards. Adding a scenario is adding a file.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::fake_crowbot::{Call, Fake, Reply};
use crate::{Sandbox, WITH_KEY, tests_path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    description: String,
    args: Vec<String>,
    /// Runs with the fake's key; `false` runs with whatever the home stores.
    #[serde(default = "yes")]
    key: bool,
    #[serde(default)]
    project: Option<String>,
    /// Files to put under crowbot's home first, by path relative to it.
    #[serde(default)]
    home_files: BTreeMap<String, String>,
    /// Files to put in the project first, over `project`'s, by path relative to it.
    #[serde(default)]
    project_files: BTreeMap<String, String>,
    /// Files to put in the user's own folder (HOME, where other apps keep their settings).
    #[serde(default)]
    user_files: BTreeMap<String, String>,
    /// The model's turns, in order.
    #[serde(default)]
    reply: Vec<Turn>,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    exit: i32,
    /// The whole of stdout.
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stdout_contains: Vec<String>,
    #[serde(default)]
    stdout_lacks: Vec<String>,
    #[serde(default)]
    stderr_contains: Vec<String>,
    #[serde(default)]
    stderr_lacks: Vec<String>,
    /// Exact contents of files in the project afterwards.
    #[serde(default)]
    files: BTreeMap<String, String>,
    #[serde(default)]
    requests: Vec<RequestCheck>,
    /// The tools a request offered the model, by name.
    #[serde(default)]
    tools: Vec<ToolsCheck>,
    /// Requests the fake served, exactly, by path.
    #[serde(default)]
    hits: BTreeMap<String, usize>,
    /// At least this many; for counts that follow cffetch's own retries.
    #[serde(default)]
    min_hits: BTreeMap<String, usize>,
}

/// One answer to a chat request: what the model says, the calls it makes, a raw fixture under
/// tests/fixtures/sse/, an error envelope, or an error whose body never ends.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Turn {
    Text(String),
    Calls(Vec<Call>),
    Sse(String),
    Error {
        status: u16,
        kind: String,
        #[serde(default)]
        retry_after: Option<u64>,
    },
    Stall {
        status: u16,
    },
}

impl Turn {
    fn reply(&self) -> Reply {
        match self {
            Self::Text(text) => Reply::Text(text.clone()),
            Self::Calls(calls) => Reply::Calls(calls.clone()),
            Self::Sse(name) => Reply::Sse(name.clone()),
            Self::Error {
                status,
                kind,
                retry_after,
            } => Reply::Error {
                status: *status,
                kind: kind.clone(),
                retry_after: *retry_after,
            },
            Self::Stall { status } => Reply::Stall { status: *status },
        }
    }
}

fn yes() -> bool {
    true
}

/// The tools chat request `index` offered: exactly `names`, in order, and among them `has` and
/// none of `lacks`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolsCheck {
    index: usize,
    #[serde(default)]
    names: Option<Vec<String>>,
    #[serde(default)]
    has: Vec<String>,
    #[serde(default)]
    lacks: Vec<String>,
}

/// A check on one chat request the model was sent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestCheck {
    index: usize,
    /// Which message in it; negative counts from the end.
    message: i64,
    role: String,
    #[serde(default)]
    contains: Option<String>,
    /// What must not reach the model there.
    #[serde(default)]
    lacks: Option<String>,
}

#[tokio::test(flavor = "multi_thread")]
async fn scenarios() {
    let dir = tests_path("fixtures/scenarios");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    // All at once, each in its own task, so one failure neither stops nor hides the rest.
    let runs = files.into_iter().map(|file| {
        let name = file.file_stem().unwrap().to_string_lossy().into_owned();
        let scenario: Scenario = toml::from_str(&std::fs::read_to_string(&file).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        tokio::spawn(check(name, scenario))
    });
    let failures: Vec<String> = futures_util::future::join_all(runs)
        .await
        .into_iter()
        .filter_map(|done| {
            let panic = done.err()?.into_panic();
            let why = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()));
            Some(why.unwrap_or_else(|| "a scenario panicked".to_owned()))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} scenario(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

async fn check(name: String, scenario: Scenario) {
    let fake = Fake::start().await;
    fake.script(scenario.reply.iter().map(Turn::reply));
    let sandbox = Sandbox::default();
    if let Some(project) = &scenario.project {
        sandbox.copy_project(project);
    }
    fake.set_home(sandbox.home());
    // `{fake_mcp}` in a file is the stand-in MCP server, with `/` so it reads in TOML;
    // `{fake_mcp_url}` the fake's own MCP endpoint over HTTP.
    let fake_mcp = env!("CARGO_BIN_EXE_crowbot-fake-mcp").replace('\\', "/");
    let fake_mcp_url = format!("{}/mcp", fake.url);
    // `{fixtures}` is tests/fixtures, for a simulated server's file.
    let fixtures = tests_path("fixtures")
        .display()
        .to_string()
        .replace('\\', "/");
    let placed = |text: &str| {
        text.replace("{fake_mcp}", &fake_mcp)
            .replace("{fake_mcp_url}", &fake_mcp_url)
            .replace("{fixtures}", &fixtures)
    };
    for (path, text) in &scenario.home_files {
        sandbox.write_home(path, &placed(text));
    }
    for (path, text) in &scenario.project_files {
        sandbox.write_project(path, &placed(text));
    }
    for (path, text) in &scenario.user_files {
        sandbox.write_user(path, &placed(text));
    }
    let args: Vec<&str> = scenario.args.iter().map(String::as_str).collect();
    let key = if scenario.key { WITH_KEY } else { None };
    let run = sandbox.run(&fake.url, &args, key).await;
    let (stdout, stderr) = (run.stdout(), run.stderr());
    let context = format!(
        "scenario {name} ({})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        scenario.description
    );
    let expect = &scenario.expect;

    // `{base}` in what is expected is the fake's address, which crowbot names as its site.
    let filled = |want: &str| want.replace("{base}", &fake.url);
    assert_eq!(run.code(), Some(expect.exit), "{context}");
    if let Some(want) = &expect.stdout {
        assert_eq!(&stdout, want, "stdout\n{context}");
    }
    for want in expect.stdout_contains.iter().map(|w| filled(w)) {
        assert!(stdout.contains(&want), "stdout lacks {want:?}\n{context}");
    }
    for want in expect.stdout_lacks.iter().map(|w| filled(w)) {
        assert!(!stdout.contains(&want), "stdout has {want:?}\n{context}");
    }
    for want in expect.stderr_contains.iter().map(|w| filled(w)) {
        assert!(stderr.contains(&want), "stderr lacks {want:?}\n{context}");
    }
    for unwanted in expect.stderr_lacks.iter().map(|w| filled(w)) {
        assert!(
            !stderr.contains(&unwanted),
            "stderr has {unwanted:?}\n{context}"
        );
    }
    for (path, want) in &expect.files {
        let got = std::fs::read_to_string(sandbox.project().join(path))
            .unwrap_or_else(|_| panic!("{path} missing\n{context}"));
        assert_eq!(&got, want, "{path}\n{context}");
    }
    let bodies = fake.chat_bodies();
    assert_eq!(
        bodies.len(),
        scenario.reply.len(),
        "requests made\n{context}"
    );
    for (path, want) in &expect.hits {
        assert_eq!(fake.hits(path), *want, "hits on {path}\n{context}");
    }
    for (path, least) in &expect.min_hits {
        let got = fake.hits(path);
        assert!(
            got >= *least,
            "{got} hits on {path}, want at least {least}\n{context}"
        );
    }
    for check in &expect.tools {
        let names: Vec<&str> = bodies
            .get(check.index)
            .and_then(|b| b["tools"].as_array())
            .unwrap_or_else(|| panic!("request {}: no tools\n{context}", check.index))
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        let at = format!("request {} tools {names:?}", check.index);
        if let Some(want) = &check.names {
            assert_eq!(&names, want, "{at}\n{context}");
        }
        for want in &check.has {
            assert!(
                names.contains(&want.as_str()),
                "{at} lack {want}\n{context}"
            );
        }
        for unwanted in &check.lacks {
            assert!(
                !names.contains(&unwanted.as_str()),
                "{at} have {unwanted}\n{context}"
            );
        }
    }
    for check in &expect.requests {
        let at = format!("request {} message {}", check.index, check.message);
        assert!(
            check.contains.is_some() || check.lacks.is_some(),
            "{at} checks nothing\n{context}"
        );
        let messages = bodies
            .get(check.index)
            .and_then(|b| b["messages"].as_array())
            .unwrap_or_else(|| panic!("{at}: no such request\n{context}"));
        let index = if check.message < 0 {
            messages.len() as i64 + check.message
        } else {
            check.message
        };
        let message: &Value = usize::try_from(index)
            .ok()
            .and_then(|i| messages.get(i))
            .unwrap_or_else(|| panic!("{at}: no such message\n{context}"));
        assert_eq!(message["role"], check.role, "{at}\n{context}");
        let content = message["content"].as_str().unwrap_or_default();
        if let Some(want) = &check.contains {
            assert!(
                content.contains(want),
                "{at} lacks {want:?}: {content}\n{context}"
            );
        }
        if let Some(unwanted) = &check.lacks {
            assert!(
                !content.contains(unwanted),
                "{at} has {unwanted:?}: {content}\n{context}"
            );
        }
    }
}
