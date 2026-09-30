//! Every tests/fixtures/scenarios/*.toml, run by one runner: a prompt, a mode, scripted model
//! replies, and what must be true afterwards. Adding a scenario is adding a file.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::fake_crowbot::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    description: String,
    args: Vec<String>,
    #[serde(default)]
    project: Option<String>,
    /// Files to put under crowbot's home first, by path relative to it.
    #[serde(default)]
    home_files: BTreeMap<String, String>,
    /// SSE fixtures under tests/fixtures/sse/, one per model turn.
    replies: Vec<String>,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    exit: i32,
    #[serde(default)]
    stdout_contains: Vec<String>,
    #[serde(default)]
    stderr_contains: Vec<String>,
    /// Exact contents of files in the project afterwards.
    #[serde(default)]
    files: BTreeMap<String, String>,
    #[serde(default)]
    requests: Vec<RequestCheck>,
    /// Requests the fake served, exactly, by path.
    #[serde(default)]
    hits: BTreeMap<String, usize>,
    /// At least this many; for counts that follow cffetch's own retries.
    #[serde(default)]
    min_hits: BTreeMap<String, usize>,
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
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scenarios");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for file in files {
        let name = file.file_stem().unwrap().to_string_lossy().into_owned();
        let scenario: Scenario = toml::from_str(&std::fs::read_to_string(&file).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        check(&name, &scenario).await;
    }
}

async fn check(name: &str, scenario: &Scenario) {
    let fake = Fake::start().await;
    fake.script(scenario.replies.iter().map(|r| Reply::sse(r)));
    let sandbox = Sandbox::default();
    if let Some(project) = &scenario.project {
        sandbox.copy_project(project);
    }
    fake.set_home(sandbox.home());
    for (path, text) in &scenario.home_files {
        let path = sandbox.home().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let args: Vec<&str> = scenario.args.iter().map(String::as_str).collect();
    let run = sandbox.run(&fake.url, &args, WITH_KEY).await;
    let (stdout, stderr) = (run.stdout(), run.stderr());
    let context = format!(
        "scenario {name} ({})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        scenario.description
    );
    let expect = &scenario.expect;

    assert_eq!(run.code(), Some(expect.exit), "{context}");
    for want in &expect.stdout_contains {
        assert!(stdout.contains(want), "stdout lacks {want:?}\n{context}");
    }
    for want in &expect.stderr_contains {
        assert!(stderr.contains(want), "stderr lacks {want:?}\n{context}");
    }
    for (path, want) in &expect.files {
        let got = std::fs::read_to_string(sandbox.project.path().join(path))
            .unwrap_or_else(|_| panic!("{path} missing\n{context}"));
        assert_eq!(&got, want, "{path}\n{context}");
    }
    let bodies = fake.chat_bodies();
    assert_eq!(
        bodies.len(),
        scenario.replies.len(),
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
