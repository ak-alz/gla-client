//! Health-check-gated apply: stages the new binary, asks a caller-
//! supplied `HealthCheck` whether the result is good, and automatically
//! commits or rolls back based on the answer — "При failure
//! автоматически восстановить предыдущую версию" (§10) is enforced
//! HERE, unconditionally, not left to the caller to remember.
//!
//! What actually constitutes "healthy" (per §10: "процесс стартовал,
//! queue доступна, config прочитан, pairing валиден, collector и
//! uploader живы") requires launching and observing a real OS process
//! — this crate stays platform-agnostic (same boundary
//! `update-manifest` already drew) by taking that as a caller-supplied
//! trait rather than implementing process supervision itself.

use crate::staging::{Staging, StagingError};
use std::path::Path;

pub trait HealthCheck {
    /// Called once, after the new binary is staged and (by whatever
    /// mechanism the caller uses — out of scope here) has been given a
    /// chance to start up. Returning `false` triggers an automatic,
    /// unconditional rollback.
    fn is_healthy(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    RolledBack,
}

/// Stages `new_binary_path`, asks `health_check`, and commits or rolls
/// back accordingly — never leaves the installation in the
/// ambiguous "staged but unconfirmed" state that
/// `Staging::has_pending_interrupted_apply` is for detecting a CRASH
/// mid-apply, not a normal, completed one.
pub fn apply_with_health_check(
    staging: &Staging,
    new_binary_path: &Path,
    health_check: &dyn HealthCheck,
) -> Result<ApplyOutcome, StagingError> {
    staging.stage_and_swap(new_binary_path)?;

    if health_check.is_healthy() {
        staging.commit()?;
        Ok(ApplyOutcome::Applied)
    } else {
        staging.rollback()?;
        Ok(ApplyOutcome::RolledBack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct FixedHealthCheck(bool);
    impl HealthCheck for FixedHealthCheck {
        fn is_healthy(&self) -> bool {
            self.0
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("updater-health-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_healthy_new_version_is_committed_and_stays_active() {
        let dir = temp_dir("healthy");
        std::fs::write(dir.join("agent.bin"), b"old").unwrap();
        std::fs::write(dir.join("new"), b"new, healthy").unwrap();

        let staging = Staging::new(&dir, "agent.bin");
        let outcome =
            apply_with_health_check(&staging, &dir.join("new"), &FixedHealthCheck(true)).unwrap();

        assert_eq!(outcome, ApplyOutcome::Applied);
        assert_eq!(
            std::fs::read(dir.join("agent.bin")).unwrap(),
            b"new, healthy"
        );
        assert!(!dir.join("agent.bin.rollback").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unhealthy_new_version_is_automatically_rolled_back() {
        let dir = temp_dir("unhealthy");
        std::fs::write(dir.join("agent.bin"), b"old, known-good").unwrap();
        std::fs::write(dir.join("new"), b"new, broken").unwrap();

        let staging = Staging::new(&dir, "agent.bin");
        let outcome =
            apply_with_health_check(&staging, &dir.join("new"), &FixedHealthCheck(false)).unwrap();

        assert_eq!(outcome, ApplyOutcome::RolledBack);
        assert_eq!(
            std::fs::read(dir.join("agent.bin")).unwrap(),
            b"old, known-good"
        );
        assert!(!dir.join("agent.bin.rollback").exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}
