//! The installer scripts (install/) against a stand-in for a release's downloads, which serves
//! this build of crowbot under every asset's name.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::routing::get;
use sha2::{Digest, Sha256};

use crate::cleared;

/// Every file a release lists for a platform, by data/release.toml.
fn asset_names() -> Vec<String> {
    let release: toml::Table = toml::from_str(include_str!("../../data/release.toml")).unwrap();
    release["asset"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["file"].as_str().unwrap().to_owned())
        .collect()
}

/// Serves `program` under every asset's name, with a SHA256SUMS that gives `summed`'s hash for
/// each; returns the base the scripts download from.
async fn downloads(program: Vec<u8>, summed: &[u8]) -> String {
    let hex: String = Sha256::digest(summed)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut sums = String::new();
    for name in asset_names() {
        sums.push_str(&format!("{hex}  {name}\n"));
        files.insert(name, program.clone());
    }
    files.insert("SHA256SUMS".into(), sums.into_bytes());
    let files = Arc::new(files);
    let app = Router::new().route(
        "/{name}",
        get(move |UrlPath(name): UrlPath<String>| {
            let files = files.clone();
            async move { files.get(&name).cloned().ok_or(StatusCode::NOT_FOUND) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// install/<name>, the script a release publishes.
fn script(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("install")
        .join(name)
}

fn this_build() -> Vec<u8> {
    std::fs::read(env!("CARGO_BIN_EXE_crowbot")).unwrap()
}

/// Runs `program args` for the installer, with `home` as the user's home and nothing of the
/// developer's crowbot setup.
async fn installer(program: &str, args: &[&Path], base: &str, home: &Path) -> std::process::Output {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env("CROWBOT_DOWNLOAD_URL", base)
        .env("CROWBOT_HOME", home.join(".crowbot"))
        .env("HOME", home);
    for name in cleared() {
        command.env_remove(name);
    }
    tokio::time::timeout(crate::common::RUN_LIMIT, command.output())
        .await
        .expect("the installer finished in time")
        .unwrap()
}

#[cfg(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
mod sh {
    use super::*;

    async fn install_sh(base: &str, home: &Path) -> std::process::Output {
        installer("sh", &[&script("install.sh")], base, home).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn install_sh_puts_a_checked_crowbot_in_the_users_bin() {
        let program = this_build();
        let base = downloads(program.clone(), &program).await;
        let home = tempfile::tempdir().unwrap();
        let out = install_sh(&base, home.path()).await;
        let said = String::from_utf8_lossy(&out.stderr).into_owned()
            + &String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{said}");
        assert!(said.contains("Installed to"), "{said}");
        let installed = home.path().join(".local/bin/crowbot");
        let version = std::process::Command::new(&installed)
            .arg("--version")
            .output()
            .unwrap();
        let version = String::from_utf8_lossy(&version.stdout);
        assert!(version.contains(env!("CARGO_PKG_VERSION")), "{version}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn install_sh_installs_nothing_that_does_not_match_its_sum() {
        let base = downloads(this_build(), b"another build").await;
        let home = tempfile::tempdir().unwrap();
        let out = install_sh(&base, home.path()).await;
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{said}");
        assert!(said.contains("does not match SHA256SUMS"), "{said}");
        assert!(!home.path().join(".local/bin/crowbot").exists());
    }
}

/// Installs for the real user (their LocalAppData, their PATH), so only on a CI runner, which is
/// thrown away; it uninstalls after.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn install_ps1_installs_a_checked_crowbot_for_the_user() {
    if std::env::var_os("CI").is_none() {
        return;
    }
    let program = this_build();
    let base = downloads(program.clone(), &program).await;
    let home = tempfile::tempdir().unwrap();
    let script = script("install.ps1");
    let flags: [&Path; 5] = [
        Path::new("-NoProfile"),
        Path::new("-ExecutionPolicy"),
        Path::new("Bypass"),
        Path::new("-File"),
        &script,
    ];
    let out = installer("powershell", &flags, &base, home.path()).await;
    let said =
        String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}");
    let local = std::env::var("LOCALAPPDATA").unwrap();
    let installed = Path::new(&local).join("Programs/crowbot/crowbot.exe");
    assert!(installed.exists(), "{said}");
    let removed = std::process::Command::new(&installed)
        .arg("uninstall")
        .env("CROWBOT_HOME", home.path().join(".crowbot"))
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
}
