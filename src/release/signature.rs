//! Whether a release's checksums are crowbot's: a minisign signature by one of the keys in
//! data/release.toml, whose trusted comment names that release.
// CEILING: the secret key is a CI secret, so whoever can read the repository's secrets can sign a
// release. Upgrade: sign on the owner's own machine once CI has built the files.

use minisign_verify::{PublicKey, Signature};

use super::Signing;
use crate::text::template::fill;

/// Whether `signature` (a .minisig file's text) signs `sums` as release `tag`, by a key `signing`
/// names. Only prehashed signatures count, as minisign has made since 0.8.
pub fn check(sums: &[u8], signature: &str, tag: &str, signing: &Signing) -> bool {
    let Ok(signature) = Signature::decode(signature) else {
        return false;
    };
    let comment = fill(&signing.comment, &[("tag", tag)]);
    signature.trusted_comment() == comment
        && signing.keys.iter().any(|key| {
            PublicKey::from_base64(key).is_ok_and(|key| key.verify(sums, &signature, false).is_ok())
        })
}

/// A key of its own, to sign stand-in releases with.
#[cfg(test)]
pub struct TestKey {
    pair: minisign::KeyPair,
    /// How the updater is told to check: this key alone, the comment as released.
    pub signing: Signing,
}

#[cfg(test)]
impl TestKey {
    pub fn new() -> Self {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let signing = Signing {
            file: super::signing().file.clone(),
            keys: vec![pair.pk.to_base64()],
            comment: super::signing().comment.clone(),
        };
        Self { pair, signing }
    }

    /// `sums` signed as release `tag`, as release.yml signs them.
    pub fn sign(&self, sums: &str, tag: &str) -> String {
        let comment = fill(&self.signing.comment, &[("tag", tag)]);
        minisign::sign(
            Some(&self.pair.pk),
            &self.pair.sk,
            std::io::Cursor::new(sums.as_bytes()),
            Some(&comment),
            None,
        )
        .unwrap()
        .into_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_counts_only_for_its_own_bytes_key_and_release() {
        let key = TestKey::new();
        let sums = "abc  crowbot-x\n";
        let signed = key.sign(sums, "v0.2.0-9");
        assert!(check(sums.as_bytes(), &signed, "v0.2.0-9", &key.signing));
        assert!(!check(
            b"abd  crowbot-x\n",
            &signed,
            "v0.2.0-9",
            &key.signing
        ));
        assert!(!check(sums.as_bytes(), &signed, "v0.2.0-10", &key.signing));
        assert!(!check(
            sums.as_bytes(),
            &signed,
            "v0.2.0-9",
            &TestKey::new().signing
        ));
        assert!(!check(
            sums.as_bytes(),
            "not a signature",
            "v0.2.0-9",
            &key.signing
        ));
    }

    #[test]
    fn any_listed_key_may_sign() {
        let (old, new) = (TestKey::new(), TestKey::new());
        let both = Signing {
            keys: [old.signing.keys.clone(), new.signing.keys.clone()].concat(),
            ..TestKey::new().signing
        };
        let sums = "abc  crowbot-x\n";
        assert!(check(sums.as_bytes(), &new.sign(sums, "v1"), "v1", &both));
        assert!(check(sums.as_bytes(), &old.sign(sums, "v1"), "v1", &both));
    }

    #[test]
    fn the_release_keys_are_minisign_public_keys() {
        assert!(!super::super::signing().keys.is_empty());
        for key in &super::super::signing().keys {
            assert!(PublicKey::from_base64(key).is_ok(), "{key}");
        }
    }
}
