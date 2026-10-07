//! Logging in to a forum, from the card and from the command line.

use super::harness::{Session, in_session};
use crate::Sandbox;
use crate::fake_crowbot::{ENV_KEY, FORUM_PASSWORD, FORUM_PATH, Fake};

/// A forum store holding one guest forum on the fake server, sealed the way a non-Windows
/// machine seals it.
fn seed_forum(sandbox: &Sandbox, fake_url: &str) {
    use base64::Engine;
    let forums = serde_json::json!([{
        "id": "1",
        "name": "Fake Forum",
        "hint": "A stand-in forum for tests.",
        "base_url": format!("{fake_url}{FORUM_PATH}"),
        "kind": "mobiquo",
    }]);
    let blob = base64::engine::general_purpose::STANDARD.encode(forums.to_string());
    let sealed = serde_json::json!({"v": 1, "scheme": "plain", "blob": blob});
    std::fs::write(sandbox.home().join("forums.json"), sealed.to_string()).unwrap();
}

/// Every file under `dir`, read as text where it is text.
fn all_text(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&all_text(&path));
        } else {
            out.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_forums_logs_in_with_a_hidden_password_and_retries_a_wrong_one() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    seed_forum(&sandbox, &fake.url);
    let ended = in_session(&fake, sandbox, Some(ENV_KEY), |s| {
        s.wait_for("MANUAL");
        s.type_text("/forums");
        s.send("\r");
        s.wait_for("╭ Forums");
        s.wait_for("Fake Forum");
        s.send("\r");
        s.wait_for("1 Log in");
        s.send("1");
        s.wait_for("Log in to Fake Forum");
        s.type_text("crow");
        s.send("\r");
        s.type_text("wrong");
        s.send("\r");
        s.wait_for("Wrong password");
        // Back at the name, still typed: Enter, then the right password.
        s.send("\r");
        s.type_text(FORUM_PASSWORD);
        s.wait_for("•••••••");
        assert!(!s.contents().contains(FORUM_PASSWORD), "{}", s.contents());
        s.send("\r");
        s.wait_for("Logged in to Fake Forum as crow.");
        s.wait_for("logged in as crow");
        s.send("\x1b");
        s.wait_gone("╭ Forums");
    })
    .await;
    assert!(!ended.screen.contains(FORUM_PASSWORD), "{}", ended.screen);
    let home = all_text(ended.sandbox.home());
    assert!(
        !home.contains(FORUM_PASSWORD),
        "the password reached the disk"
    );
    let forums = std::fs::read_to_string(ended.sandbox.home().join("forums.json")).unwrap();
    let sealed: serde_json::Value = serde_json::from_str(&forums).unwrap();
    // A DPAPI blob is opaque to the test; `io::secret`'s unit test covers its round trip.
    if cfg!(windows) {
        assert_eq!(sealed["scheme"], "dpapi");
        return;
    }
    let opened = {
        use base64::Engine;
        let blob = sealed["blob"].as_str().unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(blob)
            .unwrap()
    };
    let opened = String::from_utf8(opened).unwrap();
    assert!(opened.contains("bbsessionhash=fake"), "{opened}");
    assert!(opened.contains("\"username\":\"crow\""), "{opened}");
    assert!(!opened.contains(FORUM_PASSWORD), "{opened}");
}

#[tokio::test(flavor = "multi_thread")]
async fn forums_login_on_the_command_line_takes_a_pasted_hidden_password() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    seed_forum(&sandbox, &fake.url);
    let url = fake.url.clone();
    let screen = tokio::task::spawn_blocking(move || {
        let args = ["forums", "login", "Fake Forum"];
        let mut s = Session::start_as(&sandbox, &url, Some(ENV_KEY), &args);
        s.wait_for("Username for Fake Forum");
        s.type_text("crow");
        s.send("\r");
        s.wait_for("Password (hidden)");
        // A paste arrives whole; its line break does not end the password early.
        s.send(&format!("\x1b[200~{FORUM_PASSWORD}\n\x1b[201~"));
        s.send("\r");
        s.wait_for("Logged in to Fake Forum as crow.");
        assert!(s.wait_exit(), "{}", s.contents());
        s.contents()
    })
    .await
    .unwrap();
    assert!(!screen.contains(FORUM_PASSWORD), "{screen}");
}
