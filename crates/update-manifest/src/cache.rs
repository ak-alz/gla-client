//! A local on-disk cache of the last fetched manifest, so a client
//! doesn't need network access just to re-check "is there still an
//! update pending" (e.g. after a restart). "Safe" (this task's own
//! acceptance criterion) means: a cache file is just untrusted bytes on
//! disk until re-checked — this module re-verifies the signature on
//! EVERY load, never trusting "it was valid when I wrote it" as a
//! substitute for re-checking now. A corrupted, truncated, or tampered
//! cache file is caught and reported, never silently applied.

use crate::manifest::SignedManifest;
use ed25519_dalek::VerifyingKey;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache file I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("cache file is not valid JSON (corrupted or truncated): {0}")]
    Corrupt(serde_json::Error),
    #[error("cached manifest's signature no longer verifies — tampered on disk, or signed by a different key than the one being checked against now")]
    TamperedOrStale,
}

pub struct ManifestCache {
    path: PathBuf,
}

impl ManifestCache {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Persists an ALREADY-VERIFIED manifest. This function does not
    /// itself verify `signed` — callers must have already called
    /// `signing::verify` successfully before storing, matching the
    /// principle that this crate only ever caches what it has already
    /// established is authentic, never caches-then-verifies-later.
    ///
    /// Writes to a temp file then renames into place — the same
    /// crash-safety pattern `durable-queue` already established
    /// (`std::fs::rename` as the atomicity boundary), so a process
    /// killed mid-write never leaves a half-written cache file at the
    /// real path.
    pub fn store(&self, signed: &SignedManifest) -> Result<(), CacheError> {
        let json = serde_json::to_vec(signed).expect("SignedManifest always serializes");
        let tmp_path = self.path.with_extension("tmp");
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }

    /// Loads the cached manifest, if any, and RE-VERIFIES its signature
    /// against `verifying_key` before returning it. Returns `Ok(None)`
    /// if there is no cache file yet (a normal, expected first-run
    /// state, not an error) — matches
    /// `lifecycle::CrashMarker::previous_run_crashed`'s established
    /// "absence is a valid, meaningful state" convention.
    pub fn load(&self, verifying_key: &VerifyingKey) -> Result<Option<SignedManifest>, CacheError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(CacheError::Io(err)),
        };

        let signed: SignedManifest = serde_json::from_slice(&bytes).map_err(CacheError::Corrupt)?;
        crate::signing::verify(&signed, verifying_key).map_err(|_| CacheError::TamperedOrStale)?;
        Ok(Some(signed))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Architecture, Channel, Platform, UnsignedManifest};
    use crate::signing;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use semver::Version;

    fn temp_cache_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "update-manifest-cache-test-{name}-{}.json",
            std::process::id()
        ))
    }

    fn sample_signed(signing_key: &SigningKey) -> SignedManifest {
        let manifest = UnsignedManifest {
            version: Version::new(1, 0, 0),
            channel: Channel::Stable,
            platform: Platform::Linux,
            architecture: Architecture::X86_64,
            min_compatible_backend: Version::new(1, 0, 0),
            min_compatible_schema: Version::new(1, 0, 0),
            artifact_url: "https://example.invalid/x".to_string(),
            artifact_sha256: "a".repeat(64),
            release_notes_url: "https://example.invalid/notes".to_string(),
            rollout_percentage: 100,
            mandatory: false,
            rollback_target: None,
        };
        signing::sign(manifest, signing_key)
    }

    #[test]
    fn store_then_load_round_trips_and_reverifies() {
        let path = temp_cache_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sample_signed(&signing_key);

        let cache = ManifestCache::new(&path);
        cache.store(&signed).unwrap();

        let loaded = cache.load(&signing_key.verifying_key()).unwrap();
        assert_eq!(loaded.unwrap().manifest.version, signed.manifest.version);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn loading_a_nonexistent_cache_returns_none_not_an_error() {
        let path = temp_cache_path("missing");
        let _ = std::fs::remove_file(&path);
        let signing_key = SigningKey::generate(&mut OsRng);
        let cache = ManifestCache::new(&path);
        assert!(cache.load(&signing_key.verifying_key()).unwrap().is_none());
    }

    #[test]
    fn a_corrupted_cache_file_is_reported_not_panicked_on() {
        let path = temp_cache_path("corrupt");
        std::fs::write(&path, b"not valid json{{{").unwrap();
        let signing_key = SigningKey::generate(&mut OsRng);
        let cache = ManifestCache::new(&path);
        assert!(matches!(
            cache.load(&signing_key.verifying_key()),
            Err(CacheError::Corrupt(_))
        ));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_tampered_on_disk_cache_fails_reverification() {
        let path = temp_cache_path("tampered");
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sample_signed(&signing_key);
        let cache = ManifestCache::new(&path);
        cache.store(&signed).unwrap();

        // Simulate on-disk tampering: rewrite the file with a
        // different, unsigned rollout_percentage while keeping the
        // original (now-stale) signature bytes.
        let mut tampered = signed.clone();
        tampered.manifest.rollout_percentage = 0;
        std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();

        assert!(matches!(
            cache.load(&signing_key.verifying_key()),
            Err(CacheError::TamperedOrStale)
        ));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_cache_signed_by_a_different_key_fails_reverification() {
        let path = temp_cache_path("wrongkey");
        let signing_key = SigningKey::generate(&mut OsRng);
        let other_key = SigningKey::generate(&mut OsRng);
        let signed = sample_signed(&signing_key);
        let cache = ManifestCache::new(&path);
        cache.store(&signed).unwrap();

        assert!(matches!(
            cache.load(&other_key.verifying_key()),
            Err(CacheError::TamperedOrStale)
        ));

        std::fs::remove_file(&path).ok();
    }
}
