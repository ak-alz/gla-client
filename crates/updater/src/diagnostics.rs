//! Update status surfaced to diagnostics/tray UI — "Update status
//! visible in diagnostics" per this task's acceptance criteria. Plain
//! data, no OS dependency (same `AgentStatus`-style pattern
//! `ui-shell::status` already uses) — actually wiring this into a
//! real tray/diagnostics screen is `agent-bin`'s job.

use crate::telemetry::UpdateOutcome;
use chrono::{DateTime, Utc};
use semver::Version;

#[derive(Debug, Clone)]
pub struct UpdateDiagnostics {
    pub current_version: Version,
    /// `Some(v)` if an update to `v` has been staged
    /// (`Staging::stage_and_swap` succeeded) but not yet confirmed
    /// healthy/committed — mirrors `Staging::has_pending_interrupted_apply`
    /// at the data level, for surfacing "an update is in progress" to a
    /// human, not just to the crash-recovery code path.
    pub staged_version: Option<Version>,
    pub last_check_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<UpdateOutcome>,
    /// Whether this device currently falls within the manifest's
    /// rollout percentage (`update_manifest::is_in_rollout`) — lets a
    /// diagnostics screen honestly say "not yet offered to this
    /// device" instead of looking like the update check is broken.
    pub rollout_eligible: bool,
}

impl UpdateDiagnostics {
    /// A human-readable one-line summary — the same "small, safe,
    /// pre-composed status line" shape `ui-shell::status`'s
    /// `last_sync_line`/`pending_line` already use, so a future tray
    /// integration can display this without duplicating formatting
    /// logic.
    pub fn summary_line(&self) -> String {
        match (&self.staged_version, self.last_outcome) {
            (Some(staged), _) => {
                format!("Update to {staged} staged, waiting for a safe restart window")
            }
            (None, Some(UpdateOutcome::Applied)) => {
                format!("Running {} (up to date)", self.current_version)
            }
            (None, Some(UpdateOutcome::RolledBack)) => format!(
                "Running {} (last update was automatically rolled back)",
                self.current_version
            ),
            (None, Some(UpdateOutcome::DownloadFailed)) => format!(
                "Running {} (last update check failed to download)",
                self.current_version
            ),
            (None, Some(UpdateOutcome::ChecksumMismatch)) => format!(
                "Running {} (last update failed integrity verification)",
                self.current_version
            ),
            (None, None) => format!("Running {} (no update check yet)", self.current_version),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> UpdateDiagnostics {
        UpdateDiagnostics {
            current_version: Version::new(1, 0, 0),
            staged_version: None,
            last_check_at: None,
            last_outcome: None,
            rollout_eligible: true,
        }
    }

    #[test]
    fn a_staged_update_is_summarized_regardless_of_last_outcome() {
        let diag = UpdateDiagnostics {
            staged_version: Some(Version::new(1, 1, 0)),
            last_outcome: Some(UpdateOutcome::Applied),
            ..base()
        };
        assert!(diag.summary_line().contains("1.1.0"));
        assert!(diag.summary_line().contains("staged"));
    }

    #[test]
    fn a_rolled_back_update_is_honestly_reported_not_hidden_as_success() {
        let diag = UpdateDiagnostics {
            last_outcome: Some(UpdateOutcome::RolledBack),
            ..base()
        };
        assert!(diag.summary_line().contains("rolled back"));
    }

    #[test]
    fn no_check_yet_is_distinguishable_from_a_successful_check() {
        let never_checked = base();
        let checked_ok = UpdateDiagnostics {
            last_outcome: Some(UpdateOutcome::Applied),
            ..base()
        };
        assert_ne!(never_checked.summary_line(), checked_ok.summary_line());
    }
}
