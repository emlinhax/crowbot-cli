//! What a user does with forums beyond reading them: find and add one, log in. Each returns what
//! happened; `crowbot forums` and the `/forums` card say it in their own words.

use anyhow::Result;

use crate::config::paths::Paths;
use crate::forums::{self, Forum, directory, mobiquo, store};
use crate::io::fetch::Fetch;

/// Finds `query` (a URL or a name) in Tapatalk's directory and stores it.
pub async fn add(fetch: &Fetch, paths: &Paths, query: &str) -> Result<Forum> {
    let forum = directory::resolve(fetch, query).await?;
    store::add(paths, forum.clone())?;
    Ok(forum)
}

/// The stored forum `key` names, added from the directory first when it is only named, so there
/// is an endpoint to log in to.
pub async fn find_or_add(fetch: &Fetch, paths: &Paths, key: &str) -> Result<Forum> {
    match store::get(paths, key)? {
        Some(forum) => Ok(forum),
        None => add(fetch, paths, key).await,
    }
}

/// Logs in with the user's own account, keeping only the session cookies the forum hands back.
/// Returns the name the forum knows the user by.
pub async fn sign_in(
    fetch: &Fetch,
    paths: &Paths,
    forum: &Forum,
    user: &str,
    password: &str,
) -> Result<String> {
    let login = mobiquo::login(fetch, forum, user, password).await?;
    let name = login.username.clone();
    store::set_login(paths, forum, login)?;
    Ok(name)
}

/// The searches data/forums.toml suggests, less the forums already added.
pub fn suggestions(stored: &[Forum]) -> Vec<&'static str> {
    forums::data()
        .suggest
        .iter()
        .map(|s| s.query.as_str())
        .filter(|q| !stored.iter().any(|f| store::matches(f, q)))
        .collect()
}
