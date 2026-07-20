//! Ed25519 signing/verification over a manifest's canonical bytes.
//!
//! "Canonical" here means `serde_json::to_vec(&unsigned_manifest)` —
//! for a `#[derive(Serialize)]` struct (not a `Value`/`Map`), serde_json
//! serializes fields in the order they're declared in the struct
//! definition, not sorted or reordered, so this is deterministic across
//! runs and machines as long as `UnsignedManifest`'s field order/types
//! don't change. If they ever do, old signatures stop verifying against
//! re-serialized old manifests — an intentional, acceptable trade-off
//! (a manifest schema change should invalidate old signatures, not
//! silently keep accepting them under new field semantics), but worth
//! knowing before touching that struct.

use crate::manifest::{SignedManifest, UnsignedManifest};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

#[derive(Debug, thiserror::Error)]
pub enum VerificationError {
    #[error("manifest signature does not verify against the provided public key")]
    InvalidSignature,
    #[error("signature bytes are not a valid Ed25519 signature: {0}")]
    MalformedSignature(String),
}

fn canonical_bytes(manifest: &UnsignedManifest) -> Vec<u8> {
    serde_json::to_vec(manifest).expect("UnsignedManifest always serializes")
}

/// Signs `manifest` with `signing_key`, producing a `SignedManifest`
/// ready to publish. This is release-tooling code (AG-UPD-002/the
/// eventual manifest-publishing pipeline), not something a running
/// agent ever calls on itself — an agent only ever verifies.
pub fn sign(manifest: UnsignedManifest, signing_key: &SigningKey) -> SignedManifest {
    let signature = signing_key.sign(&canonical_bytes(&manifest));
    SignedManifest {
        manifest,
        signature: signature.to_bytes().to_vec(),
    }
}

/// Verifies `signed.signature` against `signed.manifest`'s canonical
/// bytes using `verifying_key` — the ONE gate every manifest must pass
/// before anything else in this crate (channel isolation, downgrade
/// checks, rollout bucketing) is even consulted. A manifest that fails
/// this check must be rejected outright, not degraded to "treat as
/// lower-trust" — there is no partial trust level for a manifest whose
/// authenticity can't be established (this task's own "Invalid manifest
/// rejected" acceptance criterion).
pub fn verify(
    signed: &SignedManifest,
    verifying_key: &VerifyingKey,
) -> Result<(), VerificationError> {
    let sig_bytes: [u8; 64] = signed.signature.as_slice().try_into().map_err(|_| {
        VerificationError::MalformedSignature(format!(
            "expected 64 bytes, got {}",
            signed.signature.len()
        ))
    })?;
    let signature = Signature::from_bytes(&sig_bytes);

    verifying_key
        .verify(&canonical_bytes(&signed.manifest), &signature)
        .map_err(|_| VerificationError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Architecture, Channel, Platform};
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use semver::Version;

    fn sample_manifest() -> UnsignedManifest {
        UnsignedManifest {
            version: Version::new(1, 2, 3),
            channel: Channel::Stable,
            platform: Platform::Linux,
            architecture: Architecture::X86_64,
            min_compatible_backend: Version::new(1, 0, 0),
            min_compatible_schema: Version::new(1, 0, 0),
            artifact_url: "https://example.invalid/growth-layer-agent_1.2.3.deb".to_string(),
            artifact_sha256: "a".repeat(64),
            release_notes_url: "https://example.invalid/releases/1.2.3".to_string(),
            rollout_percentage: 100,
            mandatory: false,
            rollback_target: None,
        }
    }

    #[test]
    fn a_correctly_signed_manifest_verifies() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sign(sample_manifest(), &signing_key);
        assert!(verify(&signed, &signing_key.verifying_key()).is_ok());
    }

    #[test]
    fn verification_fails_against_the_wrong_public_key() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let other_key = SigningKey::generate(&mut OsRng);
        let signed = sign(sample_manifest(), &signing_key);
        assert!(matches!(
            verify(&signed, &other_key.verifying_key()),
            Err(VerificationError::InvalidSignature)
        ));
    }

    #[test]
    fn tampering_with_any_field_after_signing_invalidates_the_signature() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sign(sample_manifest(), &signing_key);

        let mut tampered = signed.clone();
        tampered.manifest.rollout_percentage = 0; // originally 100 — an attacker "throttling" a rollout
        tampered.manifest.artifact_url =
            format!("{}-attacker-controlled", tampered.manifest.artifact_url);

        assert!(matches!(
            verify(&tampered, &signing_key.verifying_key()),
            Err(VerificationError::InvalidSignature)
        ));
    }

    #[test]
    fn malformed_signature_bytes_are_rejected_not_panicked_on() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let mut signed = sign(sample_manifest(), &signing_key);
        signed.signature = vec![0u8; 3]; // not 64 bytes
        assert!(matches!(
            verify(&signed, &signing_key.verifying_key()),
            Err(VerificationError::MalformedSignature(_))
        ));
    }

    #[test]
    fn canonical_bytes_are_deterministic_across_calls() {
        let manifest = sample_manifest();
        assert_eq!(canonical_bytes(&manifest), canonical_bytes(&manifest));
    }
}
