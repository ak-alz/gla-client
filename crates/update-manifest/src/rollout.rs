//! Deterministic rollout-percentage bucketing. "Deterministic" means:
//! the SAME `(device_id, manifest_version)` pair always maps to the
//! SAME bucket, on every call, on every machine — not re-rolled per
//! check (which would make a device flicker in and out of a rollout
//! across successive polls, and would make "rollout percentage" not
//! actually mean what it says).
//!
//! The version is part of the hash input, not just the device id —
//! otherwise the same ~N% of devices would always be the "early
//! adopters" for every single release, biasing which devices see new
//! bugs first release after release. Hashing `(device_id, version)`
//! together spreads that around per-release while staying deterministic
//! within any one release.

use semver::Version;
use sha2::{Digest, Sha256};

/// Returns `true` if this `device_id` falls within the first
/// `rollout_percentage` of devices for `manifest_version`.
/// `rollout_percentage: 0` is guaranteed to always return `false`
/// (nobody); `100` is guaranteed to always return `true` (everybody) —
/// both are exact, not approximate, regardless of `device_id`.
pub fn is_in_rollout(device_id: &str, manifest_version: &Version, rollout_percentage: u8) -> bool {
    (bucket(device_id, manifest_version) as u32) < (rollout_percentage as u32)
}

/// A stable value in `0..100` for this `(device_id, version)` pair.
fn bucket(device_id: &str, manifest_version: &Version) -> u8 {
    let mut hasher = Sha256::new();
    hasher.update(device_id.as_bytes());
    hasher.update(b":");
    hasher.update(manifest_version.to_string().as_bytes());
    let digest = hasher.finalize();
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    (value % 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn same_inputs_always_give_the_same_result() {
        let a = is_in_rollout("device-123", &v("1.2.3"), 42);
        for _ in 0..50 {
            assert_eq!(is_in_rollout("device-123", &v("1.2.3"), 42), a);
        }
    }

    #[test]
    fn zero_percent_is_always_excluded() {
        for device in [
            "a",
            "b",
            "c",
            "device-with-a-much-longer-id-string-than-usual",
        ] {
            assert!(!is_in_rollout(device, &v("1.0.0"), 0));
        }
    }

    #[test]
    fn hundred_percent_is_always_included() {
        for device in [
            "a",
            "b",
            "c",
            "device-with-a-much-longer-id-string-than-usual",
        ] {
            assert!(is_in_rollout(device, &v("1.0.0"), 100));
        }
    }

    #[test]
    fn roughly_matches_the_requested_percentage_over_many_devices() {
        let total = 10_000;
        let included = (0..total)
            .filter(|i| is_in_rollout(&format!("device-{i}"), &v("1.0.0"), 25))
            .count();
        let ratio = included as f64 / total as f64;
        assert!(
            (0.20..0.30).contains(&ratio),
            "expected roughly 25% inclusion over {total} devices, got {ratio}"
        );
    }

    #[test]
    fn different_versions_give_different_bucket_assignments() {
        // Not the SAME device always in/out across releases — otherwise
        // rollout percentage would systematically bias toward the same
        // early-adopter cohort every single release.
        let device = "device-fixed";
        let a = bucket(device, &v("1.0.0"));
        let b = bucket(device, &v("2.0.0"));
        // Not a hard guarantee for any single pair (could coincidentally
        // match), but a real, meaningful signal that the version is
        // actually part of the hash input, not ignored.
        let mut any_differ = false;
        for major in 1..30 {
            if bucket(device, &Version::new(major, 0, 0)) != a {
                any_differ = true;
                break;
            }
        }
        let _ = b;
        assert!(
            any_differ,
            "bucket should vary across versions for a fixed device id"
        );
    }
}
