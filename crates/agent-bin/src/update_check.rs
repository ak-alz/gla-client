//! Wires the already-built, already-tested `update-manifest`/`updater`
//! crates into a real, running check — this is the fix for the real
//! gap found by inspection: those two crates had no dependent in this
//! binary at all, so "Проверить обновления" in the tray sat disabled
//! forever and README.md's "signed updates with atomic rollback" claim
//! was not actually true of the shipped agent.
//!
//! Deliberately scoped to CHECKING only, not applying: this returns
//! "here's a newer version, here's where to read about it," never
//! downloads or replaces the running binary. Auto-apply (download +
//! `updater::staging`/`health`) is real, already-tested library code,
//! but wiring it up means replacing a running process's own binary out
//! from under the user — a materially bigger, riskier piece of work,
//! deliberately deferred to its own round once this check has proven
//! itself live (see the plan this was built from).
//!
//! `installed_version` here is ALWAYS the bare `CARGO_PKG_VERSION`
//! (e.g. "0.1.20"), never `AGENT_VERSION` (which appends
//! "-rust-prototype" for display purposes only) — semver treats a
//! pre-release tag as lower precedence than the same bare version, so
//! comparing a manifest's bare version (what `manifest-signer` naturally
//! produces) against an installed version that always carries a
//! pre-release suffix would make EVERY check claim an update is
//! available, even against the exact same release.

use ed25519_dalek::VerifyingKey;
use semver::Version;
use std::time::Duration;
use update_manifest::{
    verify, Architecture, CheckKind, DecisionError, InstallationContext, Platform, SignedManifest,
};

/// Public half of the keypair generated for this project — NOT a
/// secret; embedding it is exactly what lets every agent verify a
/// manifest without trusting the transport (HTTPS to GitHub) alone.
/// The matching private key lives only on the release machine, never
/// in this repo — see the plan this was built from for exactly where.
const VERIFYING_KEY_HEX: &str = "1b4fe7aa87005ba4593159a56802fd5ac033506630e4f9d99ce4d850fc154fa5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub release_notes_url: String,
}

fn verifying_key() -> VerifyingKey {
    let bytes: [u8; 32] = (0..VERIFYING_KEY_HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&VERIFYING_KEY_HEX[i..i + 2], 16).expect("VERIFYING_KEY_HEX is valid hex"))
        .collect::<Vec<u8>>()
        .try_into()
        .expect("VERIFYING_KEY_HEX is exactly 32 bytes");
    VerifyingKey::from_bytes(&bytes).expect("VERIFYING_KEY_HEX is a valid Ed25519 public key")
}

#[cfg(target_os = "windows")]
fn current_platform() -> Platform {
    Platform::Windows
}
#[cfg(target_os = "linux")]
fn current_platform() -> Platform {
    Platform::Linux
}
#[cfg(target_os = "macos")]
fn current_platform() -> Platform {
    Platform::Macos
}

/// GitHub Releases' `/latest/download/<name>` always resolves to the
/// most recent release's asset with that exact name — a stable URL
/// that needs no per-release edits, the same reasoning `manifest-
/// signer`'s output filename convention exists to support.
fn manifest_url() -> &'static str {
    #[cfg(target_os = "windows")]
    return "https://github.com/ak-alz/gla-client/releases/latest/download/manifest-windows-x86_64.json";
    #[cfg(target_os = "linux")]
    return "https://github.com/ak-alz/gla-client/releases/latest/download/manifest-linux-x86_64.json";
    #[cfg(target_os = "macos")]
    return "https://github.com/ak-alz/gla-client/releases/latest/download/manifest-macos-x86_64.json";
}

pub trait ManifestTransport {
    fn get(&self, url: &str) -> Result<String, String>;
}

pub struct UreqManifestTransport {
    agent: ureq::Agent,
}

impl UreqManifestTransport {
    pub fn new(timeout: Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new().timeout(timeout).build(),
        }
    }
}

impl ManifestTransport for UreqManifestTransport {
    fn get(&self, url: &str) -> Result<String, String> {
        self.agent
            .get(url)
            .call()
            .map_err(|_| "manifest request failed".to_string())?
            .into_string()
            .map_err(|_| "manifest response was not valid text".to_string())
    }
}

/// The real check, minus network I/O being hardcoded — every failure
/// mode (network error, bad JSON, invalid signature, wrong channel/
/// platform/downgrade, outside this device's rollout slice) returns
/// `None`, the same "quiet retry next cycle" principle
/// `run_category_overrides_loop` already established elsewhere in this
/// binary. There is no partial-trust path: an update is either fully
/// verified and applicable, or this returns nothing at all.
pub fn check_once(
    transport: &dyn ManifestTransport,
    manifest_url: &str,
    verifying_key: &VerifyingKey,
    installed_version: &Version,
    device_id: &str,
) -> Option<AvailableUpdate> {
    let body = transport.get(manifest_url).ok()?;
    let signed: SignedManifest = serde_json::from_str(&body).ok()?;

    verify(&signed, verifying_key).ok()?;

    let ctx = InstallationContext {
        installed_version: installed_version.clone(),
        channel: update_manifest::Channel::Stable,
        platform: current_platform(),
        architecture: Architecture::X86_64,
    };
    match update_manifest::decision::evaluate(&signed.manifest, &ctx, CheckKind::Routine) {
        Ok(()) => {}
        Err(DecisionError::ChannelMismatch)
        | Err(DecisionError::PlatformMismatch)
        | Err(DecisionError::ArchitectureMismatch)
        | Err(DecisionError::Downgrade { .. }) => return None,
    }

    if !update_manifest::is_in_rollout(device_id, &signed.manifest.version, signed.manifest.rollout_percentage) {
        return None;
    }

    Some(AvailableUpdate {
        version: signed.manifest.version,
        release_notes_url: signed.manifest.release_notes_url,
    })
}

/// Real entry point `agent-bin`'s startup calls — resolves this
/// platform's manifest URL and a live `ureq` transport, so callers
/// (the periodic loop, the manual tray click) never touch those
/// details directly.
pub fn check_once_live(installed_version: &Version, device_id: &str) -> Option<AvailableUpdate> {
    let transport = UreqManifestTransport::new(Duration::from_secs(10));
    check_once(&transport, manifest_url(), &verifying_key(), installed_version, device_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use update_manifest::{sign, Channel, UnsignedManifest};

    struct FixedTransport(Result<String, String>);
    impl ManifestTransport for FixedTransport {
        fn get(&self, _url: &str) -> Result<String, String> {
            self.0.clone()
        }
    }

    fn sample_manifest(version: &str, overrides: impl FnOnce(&mut UnsignedManifest)) -> UnsignedManifest {
        let mut m = UnsignedManifest {
            version: Version::parse(version).unwrap(),
            channel: Channel::Stable,
            platform: current_platform(),
            architecture: Architecture::X86_64,
            min_compatible_backend: Version::new(0, 1, 0),
            min_compatible_schema: Version::new(0, 1, 0),
            artifact_url: "https://example.invalid/artifact".to_string(),
            artifact_sha256: "a".repeat(64),
            release_notes_url: "https://example.invalid/notes".to_string(),
            rollout_percentage: 100,
            mandatory: false,
            rollback_target: None,
        };
        overrides(&mut m);
        m
    }

    fn signed_body(manifest: UnsignedManifest, signing_key: &SigningKey) -> String {
        serde_json::to_string(&sign(manifest, signing_key)).unwrap()
    }

    // A real, freshly-generated test keypair — deliberately NOT the
    // module's embedded `VERIFYING_KEY_HEX`/its real private half
    // (which lives outside the repo). `check_once` takes the verifying
    // key as a parameter specifically so tests can inject this one
    // instead, exercising the full verify→decision→rollout pipeline
    // rather than only the "wrong key" rejection path.
    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    #[test]
    fn a_newer_correctly_signed_in_rollout_manifest_is_offered() {
        let signing_key = test_signing_key();
        let body = signed_body(sample_manifest("9.9.9", |_| {}), &signing_key);
        let transport = FixedTransport(Ok(body));
        let result = check_once(
            &transport,
            "https://x.invalid",
            &signing_key.verifying_key(),
            &Version::new(0, 1, 0),
            "device-a",
        );
        assert_eq!(
            result,
            Some(AvailableUpdate {
                version: Version::new(9, 9, 9),
                release_notes_url: "https://example.invalid/notes".to_string(),
            })
        );
    }

    #[test]
    fn a_manifest_signed_by_an_unrelated_key_is_rejected() {
        let signing_key = test_signing_key();
        let other_key = SigningKey::from_bytes(&[9u8; 32]);
        let body = signed_body(sample_manifest("9.9.9", |_| {}), &signing_key);
        let transport = FixedTransport(Ok(body));
        assert_eq!(
            check_once(
                &transport,
                "https://x.invalid",
                &other_key.verifying_key(),
                &Version::new(0, 1, 0),
                "device-a"
            ),
            None
        );
    }

    #[test]
    fn a_same_or_older_version_is_rejected_as_a_downgrade() {
        let signing_key = test_signing_key();
        let body = signed_body(sample_manifest("0.1.0", |_| {}), &signing_key);
        let transport = FixedTransport(Ok(body));
        assert_eq!(
            check_once(
                &transport,
                "https://x.invalid",
                &signing_key.verifying_key(),
                &Version::new(0, 1, 0),
                "device-a"
            ),
            None
        );
    }

    #[test]
    fn a_wrong_platform_manifest_is_rejected() {
        let signing_key = test_signing_key();
        let wrong_platform = if matches!(current_platform(), Platform::Windows) {
            Platform::Linux
        } else {
            Platform::Windows
        };
        let body = signed_body(
            sample_manifest("9.9.9", |m| m.platform = wrong_platform),
            &signing_key,
        );
        let transport = FixedTransport(Ok(body));
        assert_eq!(
            check_once(
                &transport,
                "https://x.invalid",
                &signing_key.verifying_key(),
                &Version::new(0, 1, 0),
                "device-a"
            ),
            None
        );
    }

    #[test]
    fn a_manifest_outside_this_devices_rollout_slice_is_not_offered() {
        let signing_key = test_signing_key();
        let body = signed_body(sample_manifest("9.9.9", |m| m.rollout_percentage = 0), &signing_key);
        let transport = FixedTransport(Ok(body));
        assert_eq!(
            check_once(
                &transport,
                "https://x.invalid",
                &signing_key.verifying_key(),
                &Version::new(0, 1, 0),
                "any-device-at-all"
            ),
            None
        );
    }

    #[test]
    fn a_network_failure_yields_none_not_a_panic() {
        let transport = FixedTransport(Err("boom".to_string()));
        assert_eq!(
            check_once(
                &transport,
                "https://x.invalid",
                &test_signing_key().verifying_key(),
                &Version::new(0, 1, 0),
                "device-a"
            ),
            None
        );
    }

    #[test]
    fn malformed_json_yields_none_not_a_panic() {
        let transport = FixedTransport(Ok("not json{{{".to_string()));
        assert_eq!(
            check_once(
                &transport,
                "https://x.invalid",
                &test_signing_key().verifying_key(),
                &Version::new(0, 1, 0),
                "device-a"
            ),
            None
        );
    }

    #[test]
    fn the_real_embedded_verifying_key_parses_as_a_valid_ed25519_key() {
        // Doesn't (can't) prove it matches the real private key held
        // outside the repo -- just that VERIFYING_KEY_HEX is well-formed,
        // so a typo here fails loudly in CI rather than silently at
        // runtime for every real user.
        let _ = verifying_key();
    }

    /// A genuine end-to-end exercise of the REAL transport
    /// (`UreqManifestTransport`, not `FixedTransport`) against a real
    /// local HTTP server serving a real signed manifest -- everything
    /// the other tests in this module fake (the network round trip
    /// itself) is real here. This is the closest an automated test gets
    /// to "does check_once_live actually work," short of hitting the
    /// real GitHub Releases URL.
    #[test]
    fn check_once_works_over_a_real_http_round_trip() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let signing_key = test_signing_key();
        let body = signed_body(sample_manifest("9.9.9", |_| {}), &signing_key);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf); // discard the request line/headers, this is a single-shot fake server
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let transport = UreqManifestTransport::new(Duration::from_secs(5));
        let url = format!("http://{addr}/manifest.json");
        let result = check_once(&transport, &url, &signing_key.verifying_key(), &Version::new(0, 1, 0), "device-a");

        server.join().unwrap();
        assert_eq!(
            result,
            Some(AvailableUpdate {
                version: Version::new(9, 9, 9),
                release_notes_url: "https://example.invalid/notes".to_string(),
            })
        );
    }
}
