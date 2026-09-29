//! Browser emulation pool and rotation policy.
//!
//! Validated against Cloudflare's own managed challenge
//! (community.cloudflare.com, September 2026):
//!
//! ```text
//! chrome120  -> CHALLENGE (403)
//! chrome124  -> PASS (200, real page)
//! chrome131  -> PASS
//! chrome136  -> CHALLENGE (too new for CF's known-good JA3 list)
//! firefox133 -> PASS
//! safari17_0 -> PASS
//! ```
//!
//! Keep in sync with cffetch/_fingerprints.py (Python port).

use std::sync::atomic::{AtomicUsize, Ordering};

use wreq_util::{Emulation, Profile};

/// Ordered default rotation pool.
pub const DEFAULT_POOL: &[Profile] = &[
    Profile::Chrome131,
    Profile::Chrome124,
    Profile::Safari17_0,
    Profile::Firefox133,
];

/// Fingerprint selection strategy.
///
/// * `Static` (default) — always pool[0]; rotate only on challenge.
///   Maximum cookie-stickiness, minimum identity churn.
/// * `Rotate` — round-robin through the pool per request.
/// * `Random` — weighted-random pick per request from [`DIVERSITY_SET`]
///   (market-share-ish mix of modern Chrome/Safari/Firefox, all validated
///   against Cloudflare's managed challenge). Best against per-IP identity
///   tracking, but resets identity per request — see crate docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickMode {
    #[default]
    Static,
    Rotate,
    Random,
}

/// Known-good modern fingerprints for [`PickMode::Random`].
/// Every entry passed the live Tier-4 test. Weights bias toward Chrome
/// (dominant market share). Keep in sync with Python's _policy.py.
pub const DIVERSITY_SET: &[(Profile, u32)] = &[
    (Profile::Chrome131, 40),
    (Profile::Chrome124, 20),
    (Profile::Chrome123, 10),
    (Profile::Safari17_0, 12),
    (Profile::Safari18, 6),
    (Profile::Safari18_5, 4),
    (Profile::Firefox133, 5),
    (Profile::Firefox135, 3),
];

/// Selects the emulation profile for each request (cheap to clone/share).
pub struct FingerprintPicker {
    mode: PickMode,
    pool: Vec<Profile>,
    /// True when the pool came from [`Fingerprints::auto`]. Challenge
    /// fallback then always walks [`DEFAULT_POOL`] in canonical order —
    /// deterministic regardless of pick mode. Custom pools keep user order.
    auto_pool: bool,
    cursor: AtomicUsize,
}

impl FingerprintPicker {
    pub fn new(pool: Vec<Profile>, mode: PickMode) -> Self {
        Self::with_pool_kind(pool, mode, false)
    }

    pub fn with_pool_kind(pool: Vec<Profile>, mode: PickMode, auto_pool: bool) -> Self {
        Self {
            mode,
            pool,
            auto_pool,
            cursor: AtomicUsize::new(0),
        }
    }

    pub fn mode(&self) -> PickMode {
        self.mode
    }

    /// Profile for a fresh (non-retry) request.
    pub fn pick(&self) -> Profile {
        match self.mode {
            PickMode::Static => self.pool[0],
            PickMode::Rotate => {
                let i = self.cursor.fetch_add(1, Ordering::Relaxed);
                self.pool[i % self.pool.len()]
            }
            PickMode::Random => weighted_random(),
        }
    }

    /// Ordered pool to try after `failed` was challenged.
    ///
    /// Deterministic: "auto" clients always walk [`DEFAULT_POOL`] in
    /// canonical order; custom pools walk the user's own order. Before this
    /// fix the fallback in rotate/random mode drifted with the cursor.
    pub fn fallback_order(&self, failed: Profile) -> Vec<Profile> {
        let base: &[Profile] = if self.auto_pool {
            DEFAULT_POOL
        } else {
            &self.pool
        };
        base.iter().copied().filter(|p| *p != failed).collect()
    }
}

/// Market-share-ish weighted pick from [`DIVERSITY_SET`].
fn weighted_random() -> Profile {
    use rand::Rng;
    let total: u32 = DIVERSITY_SET.iter().map(|(_, w)| w).sum();
    let mut roll = rand::rng().random_range(0..total);
    for (profile, weight) in DIVERSITY_SET {
        if roll < *weight {
            return *profile;
        }
        roll -= weight;
    }
    DIVERSITY_SET[0].0 // unreachable, but total > 0 guarantees a return above
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn static_always_pool_head() {
        let p = FingerprintPicker::new(DEFAULT_POOL.to_vec(), PickMode::Static);
        assert_eq!(p.pick(), Profile::Chrome131);
        assert_eq!(p.pick(), Profile::Chrome131);
    }

    #[test]
    fn rotate_round_robins() {
        let p = FingerprintPicker::new(DEFAULT_POOL.to_vec(), PickMode::Rotate);
        let seq: Vec<Profile> = (0..6).map(|_| p.pick()).collect();
        assert_eq!(seq[0], Profile::Chrome131);
        assert_eq!(seq[3], Profile::Firefox133);
        assert_eq!(seq[4], Profile::Chrome131); // wrapped
    }

    #[test]
    fn fallback_is_deterministic_for_auto_pool() {
        // Rotate mode, "auto" pool: fallback must follow DEFAULT_POOL
        // canonical order regardless of cursor position.
        let p = FingerprintPicker::with_pool_kind(DEFAULT_POOL.to_vec(), PickMode::Rotate, true);
        let _ = p.pick(); // advance cursor: next would be Chrome124
        let fb = p.fallback_order(Profile::Chrome131);
        assert_eq!(
            fb,
            vec![Profile::Chrome124, Profile::Safari17_0, Profile::Firefox133]
        );
    }

    #[test]
    fn fallback_respects_custom_pool_order() {
        let p = FingerprintPicker::with_pool_kind(
            vec![Profile::Firefox133, Profile::Chrome124, Profile::Chrome131],
            PickMode::Static,
            false,
        );
        let fb = p.fallback_order(Profile::Chrome124);
        assert_eq!(fb, vec![Profile::Firefox133, Profile::Chrome131]);
    }

    #[test]
    fn random_varies_and_stays_known_good() {
        let p = FingerprintPicker::new(DEFAULT_POOL.to_vec(), PickMode::Random);
        let known: HashSet<Profile> = DIVERSITY_SET.iter().map(|(p, _)| *p).collect();
        let picks: HashSet<Profile> = (0..24).map(|_| p.pick()).collect();
        assert!(picks.iter().all(|p| known.contains(p)));
        assert!(picks.len() > 1, "24 picks should produce variety");
    }
}

/// Fingerprint pool specification.
#[derive(Debug, Clone)]
pub struct Fingerprints(pub Vec<Profile>);

impl Fingerprints {
    /// The validated default pool: `[Chrome131, Chrome124, Safari17_0, Firefox133]`.
    pub fn auto() -> Self {
        Self(DEFAULT_POOL.to_vec())
    }

    /// A single-fingerprint pool (no rotation will occur).
    pub fn only(profile: Profile) -> Self {
        Self(vec![profile])
    }

    /// A custom pool, rotated in the given order.
    pub fn custom(profiles: Vec<Profile>) -> Self {
        Self(profiles)
    }

    /// Build the wreq emulation for a pool entry.
    pub(crate) fn emulation_of(profile: &Profile) -> Emulation {
        Emulation::builder().profile(*profile).build()
    }
}

impl Default for Fingerprints {
    fn default() -> Self {
        Self::auto()
    }
}