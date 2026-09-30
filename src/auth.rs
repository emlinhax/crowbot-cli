//! The crowbot key on this machine: `CROWBOT_API_KEY`, else `~/.crowbot/auth.json`.

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::api::account::{self, Me};
use crate::app::App;
use crate::io::{self, fs::Access};
use crate::paths::Paths;
use crate::settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    /// The 16-digit account number itself; there is no recovery if it is lost.
    Account,
    /// A per-machine key from pairing, revocable without touching the account.
    Device,
}

/// No `Debug`: it holds the secret.
pub struct Key {
    pub secret: String,
    pub origin: Origin,
}

#[derive(Debug)]
pub enum Origin {
    Env,
    File { kind: KeyKind, hint: String },
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default = "version")]
    v: u32,
    kind: KeyKind,
    hint: String,
    created: Timestamp,
    scheme: String,
    secret: String,
}

fn version() -> u32 {
    1
}

pub fn load(paths: &Paths) -> anyhow::Result<Option<Key>> {
    if let Some(secret) = settings::env("CROWBOT_API_KEY") {
        return Ok(Some(Key {
            secret: normalize(&secret),
            origin: Origin::Env,
        }));
    }
    let path = paths.auth();
    let Some(text) = io::fs::read_string(&path)? else {
        return Ok(None);
    };
    let stored: Stored = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is unreadable; `crowbot logout` removes it",
            path.display()
        )
    })?;
    let sealed = BASE64.decode(&stored.secret).context("auth.json secret")?;
    let secret = String::from_utf8(io::secret::open(&stored.scheme, &sealed)?)
        .context("auth.json secret")?;
    Ok(Some(Key {
        secret,
        origin: Origin::File {
            kind: stored.kind,
            hint: stored.hint,
        },
    }))
}

pub fn save(paths: &Paths, secret: &str, kind: KeyKind) -> anyhow::Result<()> {
    let secret = normalize(secret);
    let (scheme, sealed) = io::secret::seal(secret.as_bytes())?;
    let stored = Stored {
        v: version(),
        kind,
        hint: hint(&secret),
        created: io::clock::now(),
        scheme: scheme.to_owned(),
        secret: BASE64.encode(sealed),
    };
    io::fs::write_atomic(
        &paths.auth(),
        &serde_json::to_vec_pretty(&stored)?,
        Access::Private,
    )?;
    Ok(())
}

/// Checks `secret` with crowbot, stores it, and makes it the key this process uses from now on.
pub async fn adopt(app: &App, secret: &str, kind: KeyKind) -> anyhow::Result<Me> {
    let secret = normalize(secret);
    let me = account::me(&app.api.with_key(secret.clone()))
        .await
        .context("checking that key with crowbot")?;
    save(&app.paths, &secret, kind)?;
    app.api.set_key(Some(secret));
    Ok(me)
}

/// `Ok(false)` when there was no stored key.
pub fn remove(paths: &Paths) -> anyhow::Result<bool> {
    Ok(io::fs::remove(&paths.auth())?)
}

/// How many trailing characters of a key are ever shown.
pub const HINT_CHARS: usize = 4;

/// The last few characters, enough to tell keys apart without revealing them.
pub fn hint(secret: &str) -> String {
    let tail: String = secret
        .chars()
        .rev()
        .take(HINT_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

/// Account numbers are shown grouped with spaces; people paste them that way.
pub fn normalize(secret: &str) -> String {
    secret.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_remove_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        save(&paths, "4192 0837 5561 2094", KeyKind::Account).unwrap();
        let text = io::fs::read_string(&paths.auth()).unwrap().unwrap();
        assert!(!text.contains("4192083755612094") || cfg!(not(windows)));
        let key = load(&paths).unwrap().unwrap();
        assert_eq!(key.secret, "4192083755612094");
        assert!(
            matches!(key.origin, Origin::File { kind: KeyKind::Account, ref hint } if hint == "…2094")
        );
        assert!(remove(&paths).unwrap());
        assert!(load(&paths).unwrap().is_none());
    }
}
