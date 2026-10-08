//! The crowbot key on this machine: `CROWBOT_API_KEY`, else `~/.crowbot/auth.json`.

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::api::Api;
use crate::api::account::{self, Me};
use crate::api::pair::{self, Started};
use crate::config::paths::Paths;
use crate::io::{self, fs::Access};
use crate::text::shorten;

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
    #[serde(default = "io::secret::version")]
    v: u32,
    kind: KeyKind,
    hint: String,
    created: Timestamp,
    scheme: String,
    secret: String,
}

/// The key this process uses: the environment's, else the stored one.
pub fn load(paths: &Paths) -> anyhow::Result<Option<Key>> {
    match from_env() {
        Some(key) => Ok(Some(key)),
        None => stored(paths),
    }
}

/// Whether `CROWBOT_API_KEY` is set, and so used over any stored key.
pub fn env_shadows() -> bool {
    from_env().is_some()
}

/// `CROWBOT_API_KEY`, which wins over a stored key while it is set.
fn from_env() -> Option<Key> {
    io::env::var("CROWBOT_API_KEY").map(|secret| Key {
        secret: normalize(&secret),
        origin: Origin::Env,
    })
}

/// The key saved on this machine, whatever the environment says.
pub fn stored(paths: &Paths) -> anyhow::Result<Option<Key>> {
    let path = paths.auth();
    let Some(text) = io::fs::read_string(&path)? else {
        return Ok(None);
    };
    let unreadable = || {
        format!(
            "{} is unreadable; `crowbot logout` removes it",
            path.display()
        )
    };
    let stored: Stored = serde_json::from_str(&text).with_context(unreadable)?;
    let opened = io::secret::open_b64(&stored.scheme, &stored.secret).with_context(unreadable)?;
    let secret = String::from_utf8(opened).with_context(unreadable)?;
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
    let (scheme, sealed) = io::secret::seal_b64(secret.as_bytes())?;
    let stored = Stored {
        v: io::secret::version(),
        kind,
        hint: hint(&secret),
        created: io::clock::now(),
        scheme: scheme.to_owned(),
        secret: sealed,
    };
    io::fs::write_atomic(
        &paths.auth(),
        &serde_json::to_vec_pretty(&stored)?,
        Access::Private,
    )?;
    Ok(())
}

/// Checks `secret` with crowbot, stores it, and makes it the key this process uses from now on.
pub async fn adopt(
    api: &Api,
    paths: &Paths,
    secret: &str,
    kind: KeyKind,
) -> anyhow::Result<LoggedIn> {
    let secret = normalize(secret);
    let me = account::me(&api.with_key(secret.clone()))
        .await
        .context("checking that key with crowbot")?;
    save(paths, &secret, kind)?;
    let hint = hint(&secret);
    api.set_key(Some(secret));
    Ok(LoggedIn { kind, hint, me })
}

/// A login that went through: the kind of key, its hint, and the account it opens. The secret
/// stays in auth.
pub struct LoggedIn {
    pub kind: KeyKind,
    pub hint: String,
    pub me: Me,
}

/// Pairs this machine once the user has the code `started` gives: opens the page, waits for the
/// approval, and keeps the device key.
pub async fn pair(api: &Api, paths: &Paths, started: &Started) -> anyhow::Result<LoggedIn> {
    io::proc::open_url(&pair::page());
    let key = pair::wait(api, started).await?;
    adopt(api, paths, &key, KeyKind::Device).await
}

/// Removes the stored key, and has `api` go on with the environment's key if there is one;
/// `false` when no key was stored.
pub fn forget(api: &Api, paths: &Paths) -> anyhow::Result<bool> {
    let removed = io::fs::remove(&paths.auth())?;
    api.set_key(from_env().map(|key| key.secret));
    Ok(removed)
}

/// How many trailing characters of a key are ever shown.
pub const HINT_CHARS: usize = 4;

/// The last few characters, enough to tell keys apart without revealing them.
pub fn hint(secret: &str) -> String {
    shorten::tail(secret, HINT_CHARS)
}

/// Account numbers are shown grouped with spaces; people paste them that way.
fn normalize(secret: &str) -> String {
    secret.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_forget_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        save(&paths, "4192 0837 5561 2094", KeyKind::Account).unwrap();
        let text = io::fs::read_string(&paths.auth()).unwrap().unwrap();
        assert!(!text.contains("4192083755612094") || cfg!(not(windows)));
        // `stored`, not `load`: a developer's exported CROWBOT_API_KEY must not win here.
        let key = stored(&paths).unwrap().unwrap();
        assert_eq!(key.secret, "4192083755612094");
        assert!(
            matches!(key.origin, Origin::File { kind: KeyKind::Account, ref hint } if hint == "…2094")
        );
        let http = crate::io::http::Http::new(std::time::Duration::from_secs(1)).unwrap();
        let api = Api::new(http, Some(key.secret));
        assert!(forget(&api, &paths).unwrap());
        assert!(stored(&paths).unwrap().is_none());
        // What is left is whatever the environment holds.
        assert_eq!(api.has_key(), from_env().is_some());
    }

    #[test]
    fn every_way_the_file_can_be_unreadable_names_it_and_the_fix() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        save(&paths, "4192 0837 5561 2094", KeyKind::Account).unwrap();
        let good: serde_json::Value =
            serde_json::from_str(&io::fs::read_string(&paths.auth()).unwrap().unwrap()).unwrap();
        let mut bad_secret = good.clone();
        bad_secret["secret"] = "not base64!".into();
        let mut bad_scheme = good;
        bad_scheme["scheme"] = "rot13".into();
        for broken in [
            "{".to_owned(),
            bad_secret.to_string(),
            bad_scheme.to_string(),
        ] {
            io::fs::write_atomic(&paths.auth(), broken.as_bytes(), Access::Private).unwrap();
            let err = format!("{:#}", stored(&paths).err().expect("unreadable"));
            assert!(err.contains(&paths.auth().display().to_string()), "{err}");
            assert!(err.contains("crowbot logout"), "{err}");
        }
    }
}
