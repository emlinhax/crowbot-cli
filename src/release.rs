//! Which release this binary is, and which file of a release each platform runs
//! (data/release.toml).

use std::sync::LazyLock;

use serde::Deserialize;

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    toml::from_str(include_str!("../data/release.toml"))
        .expect("data/release.toml is checked by tests")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    sums: String,
    asset: Vec<Asset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    os: String,
    arch: String,
    pub file: String,
}

/// The release tag CI built this from (`v0.2.0-3`); a development build has none.
pub fn tag() -> Option<&'static str> {
    option_env!("CROWBOT_RELEASE").filter(|t| !t.is_empty())
}

/// What `--version` prints: the package version, and the release when there is one.
pub fn version() -> &'static str {
    static VERSION: LazyLock<String> = LazyLock::new(|| match tag() {
        Some(tag) => format!("{} ({tag})", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").to_owned(),
    });
    &VERSION
}

/// The file this platform runs, if releases build one for it.
pub fn asset() -> Option<&'static Asset> {
    asset_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn asset_for(os: &str, arch: &str) -> Option<&'static Asset> {
    CATALOG.asset.iter().find(|a| a.os == os && a.arch == arch)
}

/// The checksum file every release carries.
pub fn sums() -> &'static str {
    &CATALOG.sums
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_finds_its_file() {
        assert_eq!(
            asset_for("windows", "x86_64").unwrap().file,
            "crowbot-x86_64-pc-windows-msvc.exe"
        );
        assert_eq!(
            asset_for("macos", "aarch64").unwrap().file,
            "crowbot-aarch64-apple-darwin"
        );
        assert!(asset_for("macos", "x86_64").is_none());
    }

    #[test]
    fn every_asset_is_a_target_the_release_builds() {
        let ci = include_str!("../.github/workflows/ci.yml");
        for asset in &CATALOG.asset {
            let target = asset
                .file
                .trim_start_matches("crowbot-")
                .trim_end_matches(".exe");
            assert!(
                ci.contains(target),
                "ci.yml builds no {target} for {}",
                asset.file
            );
        }
    }

    #[test]
    fn a_development_build_reports_the_plain_version() {
        if tag().is_none() {
            assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        }
    }
}
