//! `AG-UPD-002` — background download, checksum verification, atomic
//! staging/apply, and health-check-gated automatic rollback. Depends
//! on `update-manifest` (AG-UPD-001) for the signed manifest itself;
//! this crate takes an already-verified manifest as input, it does not
//! re-verify signatures (that trust decision belongs entirely to
//! `update-manifest`).
//!
//! Platform-agnostic by design, same boundary `update-manifest` and
//! `collector_core` already draw: everything here is real, tested
//! logic, but actually launching/observing the new OS process (the
//! "controlled restart" + the concrete meaning of "healthy" from
//! §10 — process started, queue reachable, config read, pairing
//! valid, collector/uploader alive) is `agent-bin`'s job via the
//! `HealthCheck` trait below, the same caller-supplies-the-real-effects
//! pattern `uploader::CycleOutcome` and `ui-shell::AgentController`
//! already use.
//!
//! The intended real flow, end to end:
//! 1. `disk_space::has_enough_free_space` — refuse to even start a
//!    download that can't possibly fit.
//! 2. `download::download_with_checksum` — background, rate-limited,
//!    checksum-verified against the manifest's `artifact_sha256`.
//! 3. `restart_policy::RestartPolicy::should_restart_now` — decide
//!    WHEN it's safe to actually apply (§10's safe-window rules) — the
//!    downloaded artifact simply waits if not yet safe.
//! 4. `health::apply_with_health_check` — stage, ask the caller's
//!    `HealthCheck`, commit or automatically roll back.

pub mod disk_space;
pub mod download;
pub mod health;
pub mod restart_policy;
pub mod staging;

pub use disk_space::has_enough_free_space;
pub use download::{
    download_with_checksum, DownloadConfig, DownloadError, DownloadTransport, UreqDownloadTransport,
};
pub use health::{apply_with_health_check, ApplyOutcome, HealthCheck};
pub use restart_policy::{RestartContext, RestartPolicy};
pub use staging::{Staging, StagingError};
