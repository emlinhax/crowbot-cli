use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::io::http::Method;
use crate::settings;

const SRC: &str = include_str!("../../data/endpoints.toml");

static CATALOG: LazyLock<Catalog> =
    LazyLock::new(|| toml::from_str(SRC).expect("data/endpoints.toml is checked by tests"));

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    origins: BTreeMap<String, Origin>,
    endpoints: BTreeMap<String, Endpoint>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    url: String,
    env: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub origin: String,
    pub method: Method,
    pub path: String,
    /// Whether the call needs the account key.
    #[serde(default)]
    pub auth: bool,
}

/// Endpoint ids are fixed strings in feature code, so an unknown one is a programming error.
pub fn get(id: &str) -> &'static Endpoint {
    CATALOG
        .endpoints
        .get(id)
        .unwrap_or_else(|| panic!("no endpoint `{id}` in data/endpoints.toml"))
}

pub fn origin(name: &str) -> String {
    let origin = &CATALOG.origins[name];
    settings::env(&origin.env)
        .unwrap_or_else(|| origin.url.clone())
        .trim_end_matches('/')
        .to_owned()
}

pub fn url(endpoint: &Endpoint, args: &[(&str, &str)]) -> String {
    let path = args
        .iter()
        .fold(endpoint.path.clone(), |path, (key, value)| {
            path.replace(&format!("{{{key}}}"), value)
        });
    format!("{}{path}", origin(&endpoint.origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_endpoint_names_a_known_origin() {
        for (id, endpoint) in &CATALOG.endpoints {
            assert!(
                CATALOG.origins.contains_key(&endpoint.origin),
                "endpoint {id} uses unknown origin {}",
                endpoint.origin
            );
        }
    }

    #[test]
    fn placeholders_are_filled() {
        let endpoint = Endpoint {
            origin: "api".into(),
            method: Method::Get,
            path: "/api/pair/{code}".into(),
            auth: false,
        };
        assert!(url(&endpoint, &[("code", "abc")]).ends_with("/api/pair/abc"));
    }
}
