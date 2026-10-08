//! Tapatalk's directory: turn a search term into forums to pick from, or a forum URL into the
//! endpoint crowbot talks to. A forum entry gives its base URL, the (possibly renamed) mobiquo dir
//! and extension; the rest of crowbot builds the XML-RPC endpoint from those.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::forums::{self, Forum, mobiquo};
use crate::io::fetch::{self, Fetch};

/// Where Tapatalk's directory answers, and the app credentials it asks for (data/forums.toml).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Directory {
    pub url: String,
    pub path: String,
    pub app_id: String,
    pub app_key: String,
}

/// Search the directory; `limit` caps the results.
pub async fn search(fetch: &Fetch, query: &str, limit: i64) -> Result<Vec<Forum>> {
    search_with(fetch, &forums::data().directory, query, limit).await
}

/// The first directory match for `query`, a forum's URL or its name, as a ready entry.
pub async fn resolve(fetch: &Fetch, query: &str) -> Result<Forum> {
    search(fetch, query, 1)
        .await?
        .into_iter()
        .next()
        .with_context(|| format!("no Tapatalk forum was found for {query}"))
}

async fn search_with(
    fetch: &Fetch,
    dir: &Directory,
    query: &str,
    limit: i64,
) -> Result<Vec<Forum>> {
    let url = format!("{}{}", dir.url.trim_end_matches('/'), dir.path);
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("key", query)
        .append_pair("per_page", &limit.to_string())
        .append_pair("compact", "1")
        .append_pair("app_id", &dir.app_id)
        .append_pair("app_key", &dir.app_key)
        .finish();
    let request = fetch::Post {
        url: &url,
        body: body.into_bytes(),
        content_type: "application/x-www-form-urlencoded",
        cookie: None,
    };
    let resp = forums::post(fetch, "the forum directory", request).await?;
    let json: Json =
        serde_json::from_slice(&resp.body).context("reading the forum directory's answer")?;
    Ok(entries(&json).iter().filter_map(from_json).collect())
}

/// The forum objects, wherever the wrapper keeps them (a bare array or under a common key).
fn entries(json: &Json) -> Vec<Json> {
    if let Some(array) = json.as_array() {
        return array.clone();
    }
    for key in ["data", "forums", "result", "results"] {
        if let Some(array) = json.get(key).and_then(Json::as_array) {
            return array.clone();
        }
    }
    Vec::new()
}

fn from_json(entry: &Json) -> Option<Forum> {
    let base_url = entry.get("url").and_then(Json::as_str)?.to_owned();
    let name = entry
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or(&base_url)
        .to_owned();
    Some(Forum {
        id: entry
            .get("id")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned(),
        name,
        hint: entry
            .get("description")
            .or_else(|| entry.get("desc"))
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned(),
        base_url,
        mobiquo_dir: entry
            .get("mobiquo_dir")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned(),
        ext: entry
            .get("ext")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned(),
        kind: mobiquo::KIND.to_owned(),
        cookies: Vec::new(),
        username: None,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::Router;
    use axum::routing::post;

    use super::*;

    #[tokio::test]
    async fn a_url_resolves_to_its_renamed_endpoint() {
        // The shape the real service returns for UnknownCheats.
        let reply = serde_json::json!([{
            "id": "79565", "name": "UnknownCheats",
            "url": "https://www.unknowncheats.me/forum",
            "mobiquo_dir": "euehhp13n8dqw77", "ext": "php",
            "type": "vb3x_5.0.9", "useragent": "", "sso": "1"
        }]);
        let base = crate::testing::serve(|_| {
            Router::new().route(
                "/search_forum_v2",
                post(move || async move { axum::Json(reply) }),
            )
        })
        .await;

        let dir = Directory {
            url: base,
            path: "/search_forum_v2".into(),
            app_id: "1".into(),
            app_key: "k".into(),
        };
        let fetch = Fetch::new(Duration::from_secs(5)).unwrap();
        let found = search_with(&fetch, &dir, "https://www.unknowncheats.me/forum", 1)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        let f = &found[0];
        assert_eq!(f.name, "UnknownCheats");
        assert_eq!(f.mobiquo_dir, "euehhp13n8dqw77");
        assert_eq!(f.ext, "php");
        assert_eq!(f.kind, "mobiquo");
    }
}
