//! Which release this binary is, and which file of a release each platform runs
//! (data/release.toml).

pub mod install;
pub mod path;
mod signature;
pub mod update;

use std::sync::LazyLock;

use serde::Deserialize;

use crate::data;

static CATALOG: LazyLock<Catalog> = data::load!("release.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    sums: String,
    signature: Signing,
    asset: Vec<Asset>,
}

/// How a release's checksums are signed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signing {
    /// The signature's file in a release, beside the checksums.
    pub file: String,
    /// minisign public keys, in base64; any one of them may sign.
    pub keys: Vec<String>,
    /// The trusted comment a signature carries; `{tag}` is its release.
    pub comment: String,
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

/// The package version from Cargo.toml (`0.2.0`); what session files record.
pub fn package() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The version people see: the release (`0.2.0-3`) when there is one, else the package version.
pub fn shown() -> &'static str {
    tag().map_or(package(), |tag| tag.trim_start_matches('v'))
}

/// What `--version` prints: the package version, and the release when there is one.
pub fn version() -> &'static str {
    static VERSION: LazyLock<String> = LazyLock::new(|| match tag() {
        Some(tag) => format!("{} ({tag})", package()),
        None => package().to_owned(),
    });
    &VERSION
}

/// The program's file name on this platform: `crowbot`, or `crowbot.exe` on Windows.
pub fn program() -> String {
    format!("{}{}", env!("CARGO_PKG_NAME"), std::env::consts::EXE_SUFFIX)
}

/// The platform this binary was built for: (os, arch), as Rust names them.
pub fn platform() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

/// The file this platform runs, if releases build one for it.
pub fn asset() -> Option<&'static Asset> {
    let (os, arch) = platform();
    asset_for(os, arch)
}

fn asset_for(os: &str, arch: &str) -> Option<&'static Asset> {
    CATALOG.asset.iter().find(|a| a.os == os && a.arch == arch)
}

/// The checksum file every release carries.
pub fn sums() -> &'static str {
    &CATALOG.sums
}

pub fn signing() -> &'static Signing {
    &CATALOG.signature
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
        let ci = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/ci.yml"
        ));
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
    fn the_release_signs_its_sums_as_the_updater_checks_them() {
        let release = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/release.yml"
        ));
        let signing = signing();
        let comment = crate::text::template::fill(&signing.comment, &[("tag", "$TAG")]);
        assert!(
            release.contains(&format!("-t \"{comment}\"")),
            "release.yml signs without the comment {comment:?}"
        );
        assert!(
            release.contains(&format!("-x dist/{}", signing.file)),
            "release.yml does not write {}",
            signing.file
        );
        assert!(release.contains(&format!("-m dist/{}", sums())));
    }

    #[test]
    fn each_installer_names_every_file_of_its_platforms() {
        let sh = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/install/install.sh"));
        let ps1 = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/install/install.ps1"));
        for asset in &CATALOG.asset {
            let (name, script) = match asset.os.as_str() {
                "windows" => ("install.ps1", ps1),
                _ => ("install.sh", sh),
            };
            assert!(
                script.contains(&asset.file),
                "{name} does not install {}",
                asset.file
            );
        }
    }

    #[test]
    fn a_development_build_reports_the_plain_version() {
        if tag().is_none() {
            assert_eq!(version(), package());
            assert_eq!(shown(), package());
        }
    }
}
