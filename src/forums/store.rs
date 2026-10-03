//! The forums on this machine: `~/.crowbot/forums.json`. The whole file is sealed (DPAPI on
//! Windows, 0600 elsewhere) because a logged-in entry holds session cookies. With no file yet, the
//! list starts from the seed in `data/forums.toml`, so there is something to browse on first run.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

use crate::forums::{self, Forum, Login};
use crate::io::{self, fs::Access};
use crate::paths::Paths;

#[derive(Serialize, Deserialize)]
struct Sealed {
    #[serde(default = "version")]
    v: u32,
    scheme: String,
    blob: String,
}

fn version() -> u32 {
    1
}

/// The stored forums, or the default seed when there is no file yet.
pub fn load(paths: &Paths) -> Result<Vec<Forum>> {
    let path = paths.forums();
    let Some(text) = io::fs::read_string(&path)? else {
        return Ok(forums::data().seed.clone());
    };
    let unreadable = || {
        format!(
            "{} is unreadable; delete it to start the forum list over",
            path.display()
        )
    };
    let sealed: Sealed = serde_json::from_str(&text).with_context(unreadable)?;
    let bytes = BASE64.decode(&sealed.blob).with_context(unreadable)?;
    let opened = io::secret::open(&sealed.scheme, &bytes).with_context(unreadable)?;
    serde_json::from_slice(&opened).with_context(unreadable)
}

fn save(paths: &Paths, forums: &[Forum]) -> Result<()> {
    let json = serde_json::to_vec(forums)?;
    let (scheme, bytes) = io::secret::seal(&json)?;
    let sealed = Sealed {
        v: version(),
        scheme: scheme.to_owned(),
        blob: BASE64.encode(bytes),
    };
    io::fs::write_atomic(
        &paths.forums(),
        &serde_json::to_vec_pretty(&sealed)?,
        Access::Private,
    )?;
    Ok(())
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Whether `key` names this forum: its id, its name, or the host of its URL (a bare host or a full
/// URL both work).
pub fn matches(forum: &Forum, key: &str) -> bool {
    let k = key.trim().trim_end_matches('/');
    let h = host(&forum.base_url);
    (!forum.id.is_empty() && forum.id == k)
        || forum.name.eq_ignore_ascii_case(k)
        || (!h.is_empty() && h.eq_ignore_ascii_case(k))
        || (!h.is_empty() && h.eq_ignore_ascii_case(&host(k)))
}

fn same(a: &Forum, b: &Forum) -> bool {
    (!a.id.is_empty() && a.id == b.id) || a.base_url == b.base_url
}

pub fn get(paths: &Paths, key: &str) -> Result<Option<Forum>> {
    Ok(load(paths)?.into_iter().find(|f| matches(f, key)))
}

/// Add a forum (or refresh an existing one's endpoint), keeping any login it already had.
pub fn add(paths: &Paths, forum: Forum) -> Result<()> {
    if forum.kind != "mobiquo" {
        bail!(
            "crowbot can only talk to Tapatalk (mobiquo) forums for now, not {:?}",
            forum.kind
        );
    }
    let mut list = load(paths)?;
    if let Some(slot) = list.iter_mut().find(|f| same(f, &forum)) {
        let (cookies, username) = (slot.cookies.clone(), slot.username.clone());
        *slot = forum;
        slot.cookies = cookies;
        slot.username = username;
    } else {
        list.push(forum);
    }
    save(paths, &list)
}

/// Store a login's cookies against the matching forum; `false` when none matched.
pub fn set_login(paths: &Paths, key: &str, login: Login) -> Result<bool> {
    let mut list = load(paths)?;
    let Some(forum) = list.iter_mut().find(|f| matches(f, key)) else {
        return Ok(false);
    };
    forum.cookies = login.cookies;
    forum.username = Some(login.username);
    save(paths, &list)?;
    Ok(true)
}

/// Drop a forum's session, staying added as a guest; `false` when none matched.
pub fn logout(paths: &Paths, key: &str) -> Result<bool> {
    let mut list = load(paths)?;
    let Some(forum) = list.iter_mut().find(|f| matches(f, key)) else {
        return Ok(false);
    };
    forum.cookies.clear();
    forum.username = None;
    save(paths, &list)?;
    Ok(true)
}

/// Forget a forum entirely; `false` when none matched.
pub fn remove(paths: &Paths, key: &str) -> Result<bool> {
    let mut list = load(paths)?;
    let before = list.len();
    list.retain(|f| !matches(f, key));
    if list.len() == before {
        return Ok(false);
    }
    save(paths, &list)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().into());
        (home, paths)
    }

    #[test]
    fn an_empty_store_starts_from_the_seed() {
        let (_home, paths) = paths();
        let list = load(&paths).unwrap();
        assert!(
            list.iter().any(|f| f.name == "UnknownCheats"),
            "the seed should enable UnknownCheats"
        );
    }

    #[test]
    fn add_login_logout_remove_round_trip() {
        let (_home, paths) = paths();
        add(
            &paths,
            Forum {
                id: "9".into(),
                name: "Test".into(),
                base_url: "https://t.example/forum".into(),
                mobiquo_dir: "m".into(),
                ext: "php".into(),
                kind: "mobiquo".into(),
                ..Forum::default()
            },
        )
        .unwrap();

        assert!(
            set_login(
                &paths,
                "t.example",
                Login {
                    username: "crow".into(),
                    cookies: vec!["bbsessionhash=abc; path=/".into()],
                },
            )
            .unwrap()
        );

        let forum = get(&paths, "https://t.example/forum").unwrap().unwrap();
        assert!(forum.username.is_some());
        assert_eq!(forum.username.as_deref(), Some("crow"));
        assert_eq!(forum.cookies, ["bbsessionhash=abc; path=/"]);

        // The cookies are sealed on disk, not sitting in clear JSON.
        let on_disk = io::fs::read_string(&paths.forums()).unwrap().unwrap();
        assert!(!on_disk.contains("bbsessionhash") || cfg!(not(windows)));

        assert!(logout(&paths, "Test").unwrap());
        assert!(get(&paths, "Test").unwrap().unwrap().username.is_none());

        assert!(remove(&paths, "t.example").unwrap());
        assert!(get(&paths, "Test").unwrap().is_none());
    }

    #[test]
    fn a_non_mobiquo_forum_is_refused() {
        let (_home, paths) = paths();
        let err = add(
            &paths,
            Forum {
                base_url: "https://d.example".into(),
                kind: "discourse".into(),
                ..Forum::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("mobiquo"), "{err}");
    }
}
