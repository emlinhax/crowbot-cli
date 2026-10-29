//! The tool-use battery (NEXT_PLAN.md M9): realistic tasks run headless against a real model, so
//! the session files say which tool patterns to make easier. `make battery-setup` clones the repos
//! and `make battery` spends real money; neither runs in CI. The tests that do run there drive the
//! same runner against the in-process fake crowbot.
// Tests arrange the outside world directly; the io-wrapper rule is for the product code.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

mod cmd;
#[path = "../common/mod.rs"]
mod common;
mod corpus;
// Shared with the e2e suite, which uses all of it.
#[allow(dead_code)]
#[path = "../e2e/fake_crowbot.rs"]
mod fake_crowbot;
mod project;
mod record;
mod run;
// The results line, shared with the analyzer so writer and reader cannot drift.
mod setup;
#[path = "../../src/battery/record.rs"]
mod shape;

use std::collections::BTreeSet;
use std::path::Path;

use futures_util::StreamExt;

use corpus::{Repo, Task, bench};
use fake_crowbot::{Fake, Reply};
use run::{Battery, PLACEHOLDERS, Target};

#[test]
fn corpus_is_well_formed() {
    let suites = corpus::suites();
    for suite in &suites {
        let settings = corpus::settings(suite);
        assert!(settings.repeats > 0 && settings.parallel > 0, "{suite}");
        for (name, limit) in settings.limits() {
            assert!(
                !limit.reason.trim().is_empty(),
                "{suite}: {name} has no reason"
            );
        }
    }
    let repos = corpus::repos();
    let ids: BTreeSet<&str> = repos.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids.len(), repos.len(), "repo ids repeat");
    for repo in &repos {
        match (&repo.fixture, &repo.url, &repo.commit) {
            (Some(fixture), None, None) => assert!(
                Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture).is_dir(),
                "{}: no fixture {fixture}",
                repo.id
            ),
            (None, Some(_), Some(commit)) => assert_eq!(commit.len(), 40, "{}: pin", repo.id),
            _ => panic!("{}: a fixture, or a url and a commit", repo.id),
        }
    }
    let modes = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/modes");
    let tasks = corpus::tasks();
    assert!(!tasks.is_empty());
    for task in &tasks {
        let at = &task.id;
        assert!(
            ids.contains(task.repo.as_str()),
            "{at}: no repo {}",
            task.repo
        );
        assert!(
            modes.join(format!("{}.toml", task.mode)).is_file(),
            "{at}: mode"
        );
        assert!(task.turns > 0, "{at}: turns");
        assert!(
            suites.contains(&task.suite),
            "{at}: no suite {}",
            task.suite
        );
        for server in &task.mcp {
            assert!(
                corpus::mcp_server(server).is_file(),
                "{at}: no server {server}"
            );
            // Headless, MANUAL declines every MCP call and PLAN all but read-only ones.
            assert_eq!(task.mode, "auto", "{at}: an MCP task runs in AUTO");
        }
        if let Some(patch) = &task.patch {
            assert!(bench().join("patches").join(patch).is_file(), "{at}: patch");
        }
        let mut rest = task.check.clone();
        for known in PLACEHOLDERS {
            rest = rest.replace(known, "");
        }
        assert!(
            !rest.contains("{reply")
                && !rest.contains("{plan")
                && !rest.contains("{checks")
                && !rest.contains("{mcp"),
            "{at}: placeholder"
        );
    }
}

fn repo(id: &str) -> Repo {
    corpus::repos().into_iter().find(|r| r.id == id).unwrap()
}

fn task(id: &str) -> Task {
    corpus::tasks().into_iter().find(|t| t.id == id).unwrap()
}

/// A battery against the fake, which knows `crow-2`, writing into `out`.
fn against(fake: &Fake, out: &Path) -> Battery {
    let settings = corpus::Settings {
        model: "crow-2".into(),
        ..corpus::settings("code")
    };
    let target = Target {
        api_url: Some(fake.url.clone()),
        key: fake_crowbot::ENV_KEY.into(),
    };
    Battery::new(settings, target, None, out.to_path_buf())
}

fn auto_fix(fake: &Fake) {
    fake.script([
        Reply::call("read", r#"{"path": "src/calc.txt"}"#),
        Reply::call(
            "edit",
            r#"{"path": "src/calc.txt", "edits": [{"old_text": "2 + 2 = 5", "new_text": "2 + 2 = 4"}]}"#,
        ),
        Reply::call("bash", r#"{"command": "cat src/calc.txt"}"#),
        Reply::text("Fixed: 2 + 2 = 4."),
    ]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_checked_and_recorded() {
    let fake = Fake::start().await;
    auto_fix(&fake);
    let out = tempfile::tempdir().unwrap();
    let battery = against(&fake, out.path());
    let record = battery.run(&repo("calc"), &task("calc-fix"), 1).await;
    assert_eq!(record.error, None);
    assert!(record.check, "{:?}", record.stopped_by);
    assert_eq!(
        (record.turns, record.stopped_by, record.exit),
        (4, None, Some(0))
    );

    let dir = out.path().join(&record.dir);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
    assert!(read("diff.patch").contains("+2 + 2 = 4"));
    assert!(read("reply.txt").contains("Fixed"));
    assert!(read("session.jsonl").contains(r#""asks":[{"permission":"edit""#));
    assert_eq!(
        read("events.jsonl")
            .lines()
            .filter(|l| l.contains("message_end"))
            .count(),
        4
    );
    let results = std::fs::read_to_string(out.path().join("results.jsonl")).unwrap();
    assert!(results.contains(r#""task":"calc-fix""#), "{results}");
    for name in [
        "diff.patch",
        "reply.txt",
        "session.jsonl",
        "events.jsonl",
        "stderr.txt",
        "check.txt",
    ] {
        assert!(!read(name).contains(fake_crowbot::ENV_KEY), "{name}");
    }

    // The analyzer reads the record back: the runner and `crowbot battery report` agree.
    let home = tempfile::tempdir().unwrap();
    let report = std::process::Command::new(env!("CARGO_BIN_EXE_crowbot"))
        .args(["battery", "report"])
        .arg(out.path())
        .env("CROWBOT_HOME", home.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&report.stdout);
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    assert!(text.contains("| calc-fix | auto | 1/1 | 4.0 |"), "{text}");
    assert!(text.contains("| edit | 1 |"), "{text}");
    assert!(out.path().join("report.md").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_task_runs_its_simulated_servers_and_keeps_their_records() {
    let fake = Fake::start().await;
    fake.script([
        Reply::call(
            "db__update",
            r#"{"table": "orders", "id": "1017", "set": {"status": "paid"}}"#,
        ),
        Reply::call(
            "db__update",
            r#"{"table": "orders", "id": 1017, "set": {"status": "paid"}}"#,
        ),
        Reply::text("Order 1017 is paid again."),
    ]);
    let out = tempfile::tempdir().unwrap();
    let battery = against(&fake, out.path());
    let record = battery
        .run(&repo("desk"), &task("mcp-db-fix-refund"), 1)
        .await;
    assert_eq!(record.error, None);
    assert!(record.check, "{:?}", record.stopped_by);
    let dir = out.path().join(&record.dir);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
    let calls = read("mcp/db.calls.jsonl");
    let outcomes: Vec<&str> = calls
        .lines()
        .map(|l| {
            if l.contains("\"outcome\":\"ok\"") {
                "ok"
            } else {
                "not ok"
            }
        })
        .collect();
    assert_eq!(outcomes, ["not ok", "ok"], "{calls}");
    assert!(read("mcp/db.json").contains("\"status\": \"paid\""));
    // The records are kept beside the project, out of the model's reach and out of its diff.
    assert!(
        !read("diff.patch").contains("calls.jsonl"),
        "{}",
        read("diff.patch")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_past_its_turn_limit_is_stopped() {
    let fake = Fake::start().await;
    // The second reply stalls and no fix follows it: on a loaded CI runner the stop came after
    // crowbot had already fetched and applied the edit.
    fake.script([
        Reply::call("read", r#"{"path": "src/calc.txt"}"#),
        Reply::Stall { status: 503 },
    ]);
    let out = tempfile::tempdir().unwrap();
    let battery = against(&fake, out.path());
    let task = Task {
        turns: 1,
        ..task("calc-fix")
    };
    let record = battery.run(&repo("calc"), &task, 1).await;
    assert_eq!(record.error, None);
    assert_eq!(record.stopped_by.as_deref(), Some("turns"));
    assert!(!record.check);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "clones the repos and runs their code; run with `make battery-setup`"]
async fn setup() {
    let repos = corpus::repos();
    for repo in &repos {
        eprintln!("battery: setting up {}", repo.id);
        setup::cache(repo).await.unwrap();
    }
    let problems = setup::verify(&repos, &corpus::tasks(), &corpus::settings("code")).await;
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "spends real money; run with `make battery`"]
async fn live() {
    let key = std::env::var("CROWBOT_SMOKE_KEY").expect("set CROWBOT_SMOKE_KEY");
    let suite = var("BATTERY_SUITE").unwrap_or_else(|| "code".into());
    let mut settings = corpus::settings(&suite);
    if let Some(repeats) = var("BATTERY_REPEATS") {
        settings.repeats = repeats.parse().expect("BATTERY_REPEATS is a number");
    }
    if let Some(model) = var("BATTERY_MODEL") {
        settings.model = model;
    }
    let only: Option<Vec<String>> =
        var("BATTERY_TASKS").map(|t| t.split(',').map(|s| s.trim().to_owned()).collect());
    let tasks: Vec<Task> = corpus::tasks()
        .into_iter()
        .filter(|t| t.suite == suite)
        .filter(|t| only.as_ref().is_none_or(|o| o.contains(&t.id)))
        .collect();
    assert!(
        !tasks.is_empty(),
        "suite {suite} has no task BATTERY_TASKS names"
    );
    let repos = corpus::repos();
    let target = Target { api_url: None, key };
    let models = live_models(&target, &settings.model).await;
    let stamp = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%S").to_string();
    let out = bench().join("runs").join(&stamp);
    std::fs::create_dir_all(&out).unwrap();
    let rev = cmd::git(
        &["rev-parse", "HEAD"],
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
    .await
    .unwrap_or_default();
    let meta = serde_json::json!({
        "suite": settings.suite,
        "model": settings.model,
        "repeats": settings.repeats,
        "tasks": tasks.len(),
        "cap_micros": settings.cap_micros.value,
        "crowbot": rev.trim(),
    });
    std::fs::write(out.join("battery.json"), meta.to_string()).unwrap();
    let parallel = settings.parallel;
    let battery = Battery::new(settings, target, Some(models), out.clone());

    // Every task once before any twice, so a run cut short by the cap still covers the corpus.
    let jobs: Vec<(u32, &Task)> = (1..=battery.settings.repeats)
        .flat_map(|rep| tasks.iter().map(move |t| (rep, t)))
        .collect();
    let total = jobs.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    futures_util::stream::iter(jobs)
        .map(|(rep, task)| {
            let (battery, repos, done) = (&battery, &repos, &done);
            async move {
                if battery.capped() {
                    return;
                }
                let repo = repos.iter().find(|r| r.id == task.repo).unwrap();
                let r = battery.run(repo, task, rep).await;
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                eprintln!(
                    "battery: [{n}/{total}] {} #{rep}: {} {} turns, ${:.4}{}{}",
                    task.id,
                    if r.check { "done" } else { "not done" },
                    r.turns,
                    r.cost_micros as f64 / 1e6,
                    r.stopped_by
                        .map(|s| format!(", stopped by {s}"))
                        .unwrap_or_default(),
                    r.error.map(|e| format!(", error: {e}")).unwrap_or_default(),
                );
            }
        })
        .buffer_unordered(parallel)
        .collect::<Vec<()>>()
        .await;
    eprintln!(
        "battery: {} spent ${:.4}; records in {}",
        stamp,
        battery.spent() as f64 / 1e6,
        out.display()
    );
}

/// The live models list, fetched once into a scratch home, for every run's home to start with.
async fn live_models(target: &Target, model: &str) -> Vec<u8> {
    let home = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_crowbot"));
    command
        .args(["models", "--refresh"])
        .current_dir(home.path())
        .stdin(std::process::Stdio::null());
    run::child_env(&mut command, target, home.path());
    let out = command.output().await.unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // crowbot falls back to its bundled list when the live one is out of reach, and caches nothing.
    let cache = std::fs::read(home.path().join("cache/models.json")).unwrap_or_else(|_| {
        panic!(
            "crowbot could not fetch the live models list, so nothing can run:\n{}",
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .last()
                .unwrap_or_default()
        )
    });
    assert!(
        String::from_utf8_lossy(&cache).contains(&format!("\"{model}\"")),
        "the live list has no {model}"
    );
    cache
}

/// A variable that is set and not empty, as `make` passes an unset one.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}
