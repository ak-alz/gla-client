//! Whether a (already signature-verified) manifest should be offered to
//! THIS particular installation. Three independent gates — channel/
//! platform/architecture isolation, and the downgrade-attack check —
//! each returns a specific reason, matching this project's established
//! "honest gap, not a silent guess" convention rather than a single
//! opaque `false`.

use crate::manifest::{Architecture, Channel, Platform, UnsignedManifest};
use semver::Version;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    #[error("manifest channel does not match this installation's subscribed channel")]
    ChannelMismatch,
    #[error("manifest platform does not match this installation's platform")]
    PlatformMismatch,
    #[error("manifest architecture does not match this installation's architecture")]
    ArchitectureMismatch,
    /// The manifest's version is not newer than what's currently
    /// installed, and this check was not an explicit, caller-initiated
    /// rollback — this is exactly the "downgrade attack" shape: a stale
    /// or malicious manifest quietly offering an old (possibly
    /// vulnerable) version as if it were a normal update. Rejected
    /// unconditionally on a `Routine` check; only `ExplicitRollback`
    /// (a deliberate, presumably human/operator-initiated action, never
    /// something a background poll decides on its own) may proceed past
    /// this gate.
    #[error("manifest version {manifest_version} is not newer than the installed version {installed_version}, and this was not an explicit rollback")]
    Downgrade {
        manifest_version: Version,
        installed_version: Version,
    },
}

/// What this specific installation is and needs, to decide whether a
/// manifest even applies to it — deliberately separate from
/// `UnsignedManifest` (which describes the UPDATE being offered, not
/// the installation being asked to consider it).
#[derive(Debug, Clone)]
pub struct InstallationContext {
    pub installed_version: Version,
    pub channel: Channel,
    pub platform: Platform,
    pub architecture: Architecture,
}

/// Distinguishes a normal, automatic "is there an update" poll from a
/// deliberate, explicit rollback request — the ONLY thing that may ever
/// let a manifest whose version isn't newer than what's installed pass
/// the downgrade check. This is a caller-supplied fact about WHY this
/// check is happening, not something derived from the manifest's own
/// data (a manifest cannot assert its own way past the downgrade gate —
/// that authority belongs entirely to whatever initiated this specific
/// check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    Routine,
    ExplicitRollback,
}

/// Evaluates whether `manifest` should be offered to `ctx`. Callers
/// MUST have already verified the manifest's signature
/// (`signing::verify`) before calling this — this function assumes
/// authenticity, it does not re-check it.
pub fn evaluate(
    manifest: &UnsignedManifest,
    ctx: &InstallationContext,
    kind: CheckKind,
) -> Result<(), DecisionError> {
    if manifest.channel != ctx.channel {
        return Err(DecisionError::ChannelMismatch);
    }
    if manifest.platform != ctx.platform {
        return Err(DecisionError::PlatformMismatch);
    }
    if manifest.architecture != ctx.architecture {
        return Err(DecisionError::ArchitectureMismatch);
    }
    if manifest.version <= ctx.installed_version && kind != CheckKind::ExplicitRollback {
        return Err(DecisionError::Downgrade {
            manifest_version: manifest.version.clone(),
            installed_version: ctx.installed_version.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(installed: &str) -> InstallationContext {
        InstallationContext {
            installed_version: Version::parse(installed).unwrap(),
            channel: Channel::Stable,
            platform: Platform::Linux,
            architecture: Architecture::X86_64,
        }
    }

    fn manifest(version: &str) -> UnsignedManifest {
        UnsignedManifest {
            version: Version::parse(version).unwrap(),
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
        }
    }

    #[test]
    fn a_newer_version_on_the_matching_channel_is_accepted() {
        assert!(evaluate(&manifest("2.0.0"), &ctx("1.0.0"), CheckKind::Routine).is_ok());
    }

    #[test]
    fn a_same_or_older_version_is_rejected_on_a_routine_check() {
        assert!(matches!(
            evaluate(&manifest("1.0.0"), &ctx("1.0.0"), CheckKind::Routine),
            Err(DecisionError::Downgrade { .. })
        ));
        assert!(matches!(
            evaluate(&manifest("0.9.0"), &ctx("1.0.0"), CheckKind::Routine),
            Err(DecisionError::Downgrade { .. })
        ));
    }

    #[test]
    fn an_older_version_is_accepted_only_under_an_explicit_rollback_check() {
        assert!(evaluate(
            &manifest("0.9.0"),
            &ctx("1.0.0"),
            CheckKind::ExplicitRollback
        )
        .is_ok());
    }

    #[test]
    fn a_mismatched_channel_is_rejected_even_for_a_newer_version() {
        let mut m = manifest("2.0.0");
        m.channel = Channel::Beta;
        assert!(matches!(
            evaluate(&m, &ctx("1.0.0"), CheckKind::Routine),
            Err(DecisionError::ChannelMismatch)
        ));
    }

    #[test]
    fn a_mismatched_platform_is_rejected() {
        let mut m = manifest("2.0.0");
        m.platform = Platform::Windows;
        assert!(matches!(
            evaluate(&m, &ctx("1.0.0"), CheckKind::Routine),
            Err(DecisionError::PlatformMismatch)
        ));
    }

    #[test]
    fn a_mismatched_architecture_is_rejected() {
        let mut m = manifest("2.0.0");
        m.architecture = Architecture::Aarch64;
        assert!(matches!(
            evaluate(&m, &ctx("1.0.0"), CheckKind::Routine),
            Err(DecisionError::ArchitectureMismatch)
        ));
    }

    #[test]
    fn channel_isolation_is_checked_before_the_downgrade_gate() {
        // A wrong-channel manifest that's ALSO an older version should
        // report the channel mismatch, not the downgrade — the caller
        // shouldn't have to guess which of two real problems is the
        // actual one when both apply.
        let mut m = manifest("0.5.0");
        m.channel = Channel::Beta;
        assert!(matches!(
            evaluate(&m, &ctx("1.0.0"), CheckKind::Routine),
            Err(DecisionError::ChannelMismatch)
        ));
    }
}
