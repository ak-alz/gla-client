//! `AG-UPD-001` — a signed, channel-aware update manifest: the data
//! shape, signature verification, channel/platform/architecture
//! isolation, downgrade-attack rejection, deterministic rollout
//! bucketing, and a safe local cache. Deliberately platform-agnostic —
//! no download, no install, no restart, no health check (that's
//! `AG-UPD-002`, which will depend on this crate the same way
//! `agent-bin` depends on `event-contract`/`durable-queue` rather than
//! reimplementing their contracts).
//!
//! The trust chain this crate establishes, end to end:
//! 1. A manifest arrives (from the network, or from the local cache).
//! 2. `signing::verify` — reject anything whose signature doesn't
//!    check out. Nothing past this point is ever consulted for an
//!    unverified manifest.
//! 3. `decision::evaluate` — reject anything for the wrong channel/
//!    platform/architecture, or that would silently downgrade the
//!    installation without an explicit, caller-declared rollback
//!    intent.
//! 4. `rollout::is_in_rollout` — even a fully valid, applicable
//!    manifest may not yet be offered to this specific device, by
//!    design (staged rollout).
//! 5. `cache::ManifestCache` — persists an already-verified manifest,
//!    and re-verifies on every load (never trusts disk bytes just
//!    because they were valid once).

pub mod cache;
pub mod decision;
pub mod manifest;
pub mod rollout;
pub mod signing;

pub use cache::{CacheError, ManifestCache};
pub use decision::{CheckKind, DecisionError, InstallationContext};
pub use manifest::{Architecture, Channel, Platform, SignedManifest, UnsignedManifest};
pub use rollout::is_in_rollout;
pub use signing::{sign, verify, VerificationError};
