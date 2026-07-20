//! End-to-end: sign → verify → decide → check rollout → cache, in the
//! same order a real caller (`AG-UPD-002`) would use this crate — proves
//! the pieces compose, not just that each passes in isolation.

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use semver::Version;
use update_manifest::{
    is_in_rollout, sign, verify, Architecture, Channel, CheckKind, InstallationContext,
    ManifestCache, Platform, UnsignedManifest,
};

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "update-manifest-integration-{name}-{}.json",
        std::process::id()
    ))
}

#[test]
fn full_pipeline_accepts_a_legitimate_update_and_offers_it_to_an_in_rollout_device() {
    let signing_key = SigningKey::generate(&mut OsRng);

    let manifest = UnsignedManifest {
        version: Version::new(2, 0, 0),
        channel: Channel::Stable,
        platform: Platform::Linux,
        architecture: Architecture::X86_64,
        min_compatible_backend: Version::new(1, 0, 0),
        min_compatible_schema: Version::new(1, 0, 0),
        artifact_url: "https://example.invalid/growth-layer-agent_2.0.0.deb".to_string(),
        artifact_sha256: "a".repeat(64),
        release_notes_url: "https://example.invalid/releases/2.0.0".to_string(),
        rollout_percentage: 100, // 100% so the test doesn't depend on which bucket a given device id lands in
        mandatory: false,
        rollback_target: Some(Version::new(1, 0, 0)),
    };

    let signed = sign(manifest, &signing_key);

    // Step 1: signature verification — the one gate everything else assumes already passed.
    verify(&signed, &signing_key.verifying_key()).expect("a genuinely signed manifest must verify");

    // Step 2: channel/platform/architecture/downgrade decision.
    let ctx = InstallationContext {
        installed_version: Version::new(1, 5, 0),
        channel: Channel::Stable,
        platform: Platform::Linux,
        architecture: Architecture::X86_64,
    };
    update_manifest::decision::evaluate(&signed.manifest, &ctx, CheckKind::Routine)
        .expect("a newer, same-channel/platform/arch manifest must be accepted");

    // Step 3: rollout gating.
    assert!(is_in_rollout(
        "device-abc-123",
        &signed.manifest.version,
        signed.manifest.rollout_percentage
    ));

    // Step 4: cache store + reload, re-verifying from disk.
    let path = temp_path("full-pipeline");
    let _ = std::fs::remove_file(&path);
    let cache = ManifestCache::new(&path);
    cache.store(&signed).expect("store must succeed");
    let reloaded = cache
        .load(&signing_key.verifying_key())
        .expect("load must succeed")
        .expect("a manifest was just stored");
    assert_eq!(reloaded.manifest.version, Version::new(2, 0, 0));
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_manifest_signed_by_an_untrusted_key_never_reaches_the_decision_stage() {
    let real_key = SigningKey::generate(&mut OsRng);
    let attacker_key = SigningKey::generate(&mut OsRng);

    let manifest = UnsignedManifest {
        version: Version::new(99, 0, 0), // an obviously "tempting" fake update
        channel: Channel::Stable,
        platform: Platform::Linux,
        architecture: Architecture::X86_64,
        min_compatible_backend: Version::new(1, 0, 0),
        min_compatible_schema: Version::new(1, 0, 0),
        artifact_url: "https://attacker.invalid/payload".to_string(),
        artifact_sha256: "b".repeat(64),
        release_notes_url: "https://attacker.invalid/notes".to_string(),
        rollout_percentage: 100,
        mandatory: false,
        rollback_target: None,
    };

    let forged = sign(manifest, &attacker_key);

    // Verification against the REAL trusted key must fail — this is the
    // gate that must stop a forged manifest before decision/rollout/cache
    // logic ever sees it.
    assert!(verify(&forged, &real_key.verifying_key()).is_err());
}
