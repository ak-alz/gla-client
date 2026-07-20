//! The manifest shape itself — every field the task description asks
//! for, and nothing else. Deliberately split into `UnsignedManifest`
//! (the actual content) and `SignedManifest` (content + detached
//! signature) rather than putting a `signature` field inside the
//! signed struct itself — a signature can't cover its own field without
//! either a canonicalization dance to exclude it or a chicken-and-egg
//! ordering bug, so keeping them structurally separate makes "what
//! exactly did the signature cover" unambiguous by construction.

use semver::Version;
use serde::{Deserialize, Serialize};

/// The release channel a manifest belongs to. A fixed enum, not a free
/// `String` — channel isolation (this task's own acceptance criterion)
/// is much harder to accidentally violate when a typo'd channel name
/// can't even deserialize, vs. silently creating a new, unintended
/// channel bucket from a `String` typo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
    Dev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    Macos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
    Aarch64,
}

/// The manifest's actual content — everything `AG-UPD-001`'s task
/// description lists, verbatim, no extra fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsignedManifest {
    pub version: Version,
    pub channel: Channel,
    pub platform: Platform,
    pub architecture: Architecture,
    pub min_compatible_backend: Version,
    pub min_compatible_schema: Version,
    pub artifact_url: String,
    /// Lowercase hex-encoded SHA-256 of the artifact — checked against
    /// the ACTUAL downloaded bytes by whatever applies this manifest
    /// (`AG-UPD-002`), not verified by this crate (which never fetches
    /// the artifact itself — see the crate-level doc comment for the
    /// scope line this draws).
    pub artifact_sha256: String,
    pub release_notes_url: String,
    /// 0-100 inclusive. `0` = nobody in this rollout is offered the
    /// update yet; `100` = everybody is.
    pub rollout_percentage: u8,
    /// Per the task description: "mandatory flag only for security
    /// emergency" — this crate does not itself enforce that the flag is
    /// ONLY ever set for a genuine emergency (that's a release-process
    /// discipline question, not something a data structure can verify)
    /// but does document the constraint here so a future
    /// manifest-generation tool has it in view.
    pub mandatory: bool,
    /// `Some(version)` marks this manifest as an authorized rollback TO
    /// that specific version — see `decision.rs`'s doc comment for
    /// exactly how this is used to distinguish a legitimate rollback
    /// from a downgrade attack (a stale or malicious manifest quietly
    /// offering an old, vulnerable version as if it were current).
    pub rollback_target: Option<Version>,
}

/// `UnsignedManifest` plus a detached Ed25519 signature over its
/// canonical serialization (see `signing.rs` for exactly what "canonical"
/// means here and why).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    pub manifest: UnsignedManifest,
    /// Raw 64-byte Ed25519 signature bytes.
    pub signature: Vec<u8>,
}
