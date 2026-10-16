//! `~/.crowbot/forums.json`, sealed whole (DPAPI on Windows, 0600 elsewhere) because logged-in
//! entries hold session cookies. Without the file, the list is the seed from `data/forums.toml`.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::paths::Paths;
use crate::forums::{self, Forum, Login, mobiquo};
use crate::io::{self, fs::Access};

#[derive(Serialize, Deserialize)]
struct Sealed {
    #[serde(default = "io::secret::version")]
    v: u32,
    scheme: String,
    blob: String,
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
    let opened = io::secret::open_b64(&sealed.scheme, &sealed.blob).with_context(unreadable)?;
    let mut forums: Vec<Forum> = serde_json::from_slice(&opened).with_context(unreadable)?;
    enrich_hints(&mut forums);
    Ok(forums)
}

/// Fills an empty hint from the matching seed, e.g. for a forum stored before hints existed.
fn enrich_hints(forums: &mut [Forum]) {
    for forum in forums.iter_mut().filter(|f| f.hint.is_empty()) {
        if let Some(seed) = forums::data().seed.iter().find(|s| {
            (!s.id.is_empty() && s.id == forum.id)
                || forums::host(&s.base_url) == forums::host(&forum.base_url)
        }) {
            forum.hint = seed.hint.clone();
        }
    }
}

fn save(paths: &Paths, forums: &[Forum]) -> Result<()> {
    let json = serde_json::to_vec(forums)?;
    let (scheme, blob) = io::secret::seal_b64(&json)?;
    let sealed = Sealed {
        v: io::secret::version(),
        scheme: scheme.to_owned(),
        blob,
    };
    io::fs::write_atomic(
        &paths.forums(),
        &serde_json::to_vec_pretty(&sealed)?,
        Access::Private,
    )?;
    Ok(())
}

/// Whether `key` names this forum: its id, its name, or the host of its URL (a bare host or a full
/// URL both work).
pub fn matches(forum: &Forum, key: &str) -> bool {
    let k = key.trim().trim_end_matches('/');
    let same_host =
        |other: &str| forums::host(&forum.base_url).is_some_and(|h| h.eq_ignore_ascii_case(other));
    (!forum.id.is_empty() && forum.id == k)
        || forum.name.eq_ignore_ascii_case(k)
        || same_host(k)
        || forums::host(k).is_some_and(|h| same_host(&h))
}

fn same(a: &Forum, b: &Forum) -> bool {
    (!a.id.is_empty() && a.id == b.id) || a.base_url == b.base_url
}

pub fn get(paths: &Paths, key: &str) -> Result<Option<Forum>> {
    Ok(load(paths)?.into_iter().find(|f| matches(f, key)))
}

/// Add a forum (or refresh an existing one's endpoint), keeping any login it already had.
pub fn add(paths: &Paths, forum: Forum) -> Result<()> {
    if forum.kind != mobiquo::KIND {
        bail!(
            "only Tapatalk (mobiquo) forums are supported, not {:?}",
            forum.kind
        );
    }
    edit(paths, |list| {
        if let Some(slot) = list.iter_mut().find(|f| same(f, &forum)) {
            let (cookies, username, hint) = (
                slot.cookies.clone(),
                slot.username.clone(),
                slot.hint.clone(),
            );
            *slot = forum;
            slot.cookies = cookies;
            slot.username = username;
            // Keep a curated hint when the new source (e.g. the directory) gave none.
            if slot.hint.is_empty() {
                slot.hint = hint;
            }
        } else {
            list.push(forum);
        }
    })
}

// The edits below take the stored forum, never a typed key: a key can match every forum on a
// host, and only this one may change.

/// Store a login's cookies against `forum`.
pub fn set_login(paths: &Paths, forum: &Forum, login: Login) -> Result<()> {
    edit(paths, |list| {
        if let Some(slot) = list.iter_mut().find(|f| same(f, forum)) {
            slot.cookies = login.cookies;
            slot.username = Some(login.username);
        }
    })
}

/// Drop `forum`'s session; it stays added as a guest.
pub fn logout(paths: &Paths, forum: &Forum) -> Result<()> {
    edit(paths, |list| {
        if let Some(slot) = list.iter_mut().find(|f| same(f, forum)) {
            slot.cookies.clear();
            slot.username = None;
        }
    })
}

/// Forget `forum` entirely.
pub fn remove(paths: &Paths, forum: &Forum) -> Result<()> {
    edit(paths, |list| list.retain(|f| !same(f, forum)))
}

fn edit(paths: &Paths, change: impl FnOnce(&mut Vec<Forum>)) -> Result<()> {
    let mut list = load(paths)?;
    change(&mut list);
    save(paths, &list)
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
    fn a_stored_forum_without_a_hint_gets_one_from_the_seed() {
        let (_home, paths) = paths();
        // Added before hints existed: same host as the seeded UnknownCheats, but no hint.
        add(
            &paths,
            Forum {
                name: "UnknownCheats".into(),
                base_url: "https://www.unknowncheats.me/forum".into(),
                mobiquo_dir: "x".into(),
                kind: "mobiquo".into(),
                ..Forum::default()
            },
        )
        .unwrap();
        let forum = get(&paths, "www.unknowncheats.me").unwrap().unwrap();
        assert!(forum.hint.contains("hacking"), "{}", forum.hint);
    }

    fn forum(id: &str, name: &str, base_url: &str) -> Forum {
        Forum {
            id: id.into(),
            name: name.into(),
            base_url: base_url.into(),
            mobiquo_dir: "m".into(),
            ext: "php".into(),
            kind: "mobiquo".into(),
            ..Forum::default()
        }
    }

    fn crow() -> Login {
        Login {
            username: "crow".into(),
            cookies: vec!["bbsessionhash=abc; path=/".into()],
        }
    }

    #[test]
    fn add_login_logout_remove_round_trip() {
        let (_home, paths) = paths();
        add(&paths, forum("9", "Test", "https://t.example/forum")).unwrap();

        let stored = get(&paths, "t.example").unwrap().unwrap();
        set_login(&paths, &stored, crow()).unwrap();

        let forum = get(&paths, "https://t.example/forum").unwrap().unwrap();
        assert!(forum.username.is_some());
        assert_eq!(forum.username.as_deref(), Some("crow"));
        assert_eq!(forum.cookies, ["bbsessionhash=abc; path=/"]);

        // The cookies are sealed on disk, not sitting in clear JSON.
        let on_disk = io::fs::read_string(&paths.forums()).unwrap().unwrap();
        assert!(!on_disk.contains("bbsessionhash") || cfg!(not(windows)));

        logout(&paths, &stored).unwrap();
        assert!(get(&paths, "Test").unwrap().unwrap().username.is_none());

        remove(&paths, &stored).unwrap();
        assert!(get(&paths, "Test").unwrap().is_none());
    }

    #[test]
    fn two_forums_on_one_host_are_changed_one_at_a_time() {
        let (_home, paths) = paths();
        add(&paths, forum("1", "Main", "https://h.example/forum")).unwrap();
        add(&paths, forum("2", "Market", "https://h.example/market")).unwrap();
        let market = get(&paths, "Market").unwrap().unwrap();

        set_login(&paths, &market, crow()).unwrap();
        assert!(get(&paths, "Main").unwrap().unwrap().username.is_none());
        assert!(get(&paths, "Market").unwrap().unwrap().username.is_some());

        remove(&paths, &market).unwrap();
        assert!(get(&paths, "Main").unwrap().is_some());
        assert!(get(&paths, "Market").unwrap().is_none());
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
