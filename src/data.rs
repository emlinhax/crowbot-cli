//! Files in data/, embedded at build time. Every one is parsed by its owner's tests, so a parse
//! failure here is a broken data file, never anything a user did.

use serde::de::DeserializeOwned;

/// The text of `data/<path>`; the path may come in pieces, as `concat!` takes them.
macro_rules! raw {
    ($($path:tt)+) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/", $($path)+))
    };
}

/// A `LazyLock` of `data/<path>` parsed as TOML into the static's type, or of its `key` table.
macro_rules! load {
    ($path:literal) => {
        std::sync::LazyLock::new(|| $crate::data::parse($crate::data::raw!($path), $path))
    };
    ($path:literal, $key:literal) => {
        std::sync::LazyLock::new(|| $crate::data::parse_key($crate::data::raw!($path), $path, $key))
    };
}

pub(crate) use {load, raw};

pub fn parse<T: DeserializeOwned>(src: &str, path: &str) -> T {
    toml::from_str(src).unwrap_or_else(|e| panic!("data/{path} is checked by tests: {e}"))
}

pub fn parse_key<T: DeserializeOwned>(src: &str, path: &str, key: &str) -> T {
    let mut table: toml::Table = parse(src, path);
    let value = table
        .remove(key)
        .unwrap_or_else(|| panic!("data/{path} has no [{key}]"));
    value
        .try_into()
        .unwrap_or_else(|e| panic!("data/{path} [{key}] is checked by tests: {e}"))
}
