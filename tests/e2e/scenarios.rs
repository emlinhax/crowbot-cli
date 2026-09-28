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
}

/// A check on one chat request the model was sent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestCheck {
    index: usize,
    /// Which message in it; negative counts from the end.
    message: i64,
    role: String,
    contains: String,
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
    for check in &expect.requests {
        let messages = bodies[check.index]["messages"].as_array().unwrap();
        let at = if check.message < 0 {
            messages.len() as i64 + check.message
        } else {
            check.message
        };
        let message: &Value = &messages[at as usize];
        assert_eq!(
            message["role"], check.role,
            "request {} message {}\n{context}",
            check.index, check.message
        );
        let content = message["content"].as_str().unwrap_or_default();
        assert!(
            content.contains(&check.contains),
            "request {} message {} lacks {:?}: {content}\n{context}",
            check.index,
            check.message,
            check.contains
        );
    }
}
