// Copied into the run's copy by the semver-add-bump check; the model never sees it.
use semver::Version;

#[test]
fn bump_patch_increments_and_clears_pre_and_build() {
    let mut version = Version::parse("1.2.3-alpha.1+build.5").unwrap();
    version.bump_patch();
    assert_eq!(version, Version::parse("1.2.4").unwrap());
}
