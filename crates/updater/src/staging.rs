//! Atomic staging/apply/rollback of the installed binary. The core
//! crash-safety guarantee: the previous binary is never deleted until
//! `commit()` is called (matching this project's own written rule,
//! "Никогда не удалять рабочую предыдущую версию до health check
//! новой" — CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md §10) — and
//! its presence at `<binary>.rollback` is itself the crash-safety
//! marker: if the process is killed between `stage_and_swap()` and
//! `commit()`/`rollback()`, that file's existence on the next startup
//! is what says "an apply was in progress and never finished," the
//! same "presence = meaningful state" pattern `lifecycle::CrashMarker`
//! already established.
//!
//! Two `std::fs::rename` calls, not one `std::fs::copy` over the live
//! binary — `rename` within the same directory is atomic on both NTFS
//! and any POSIX filesystem; `copy` is not (a crash mid-copy would
//! leave a truncated binary at the live path). The new binary is
//! staged into a temp file in the SAME directory first specifically so
//! the final rename is same-filesystem (cross-filesystem renames
//! silently fall back to copy on some platforms, losing atomicity).

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("a previous apply is still pending (rollback file exists) — resolve it before starting a new one")]
    InterruptedApplyPending,
    #[error("nothing to roll back to — no rollback file exists")]
    NothingToRollBackTo,
}

pub struct Staging {
    install_dir: PathBuf,
    binary_name: String,
}

impl Staging {
    /// `binary_name` is expected to be a fixed, code-chosen constant
    /// (e.g. `"growth-layer-agent"`), never derived from anything
    /// external — enforced here, not just assumed, since
    /// `install_dir.join(binary_name)` would otherwise let a `..`
    /// component or an absolute path escape `install_dir` entirely
    /// (independent review flagged this as unexercised-but-real; no
    /// caller passes anything but a hardcoded constant today, and this
    /// keeps it that way structurally rather than by convention alone).
    pub fn new(install_dir: impl Into<PathBuf>, binary_name: impl Into<String>) -> Self {
        let binary_name = binary_name.into();
        assert!(
            !binary_name.is_empty()
                && !binary_name.contains('/')
                && !binary_name.contains('\\')
                && binary_name != "."
                && binary_name != "..",
            "Staging::new: binary_name must be a plain file name, not a path: {binary_name:?}"
        );
        Self {
            install_dir: install_dir.into(),
            binary_name,
        }
    }

    fn active_path(&self) -> PathBuf {
        self.install_dir.join(&self.binary_name)
    }

    fn rollback_path(&self) -> PathBuf {
        self.install_dir
            .join(format!("{}.rollback", self.binary_name))
    }

    fn staged_tmp_path(&self) -> PathBuf {
        self.install_dir
            .join(format!("{}.staged", self.binary_name))
    }

    /// `true` if an apply was started but never reached `commit()` or
    /// `rollback()` — e.g. the process was killed (power loss) in
    /// between. The caller (agent-bin, at startup) should check this
    /// and call `rollback()` before doing anything else if so: an
    /// update that never confirmed its own health should never be
    /// assumed good just because the process happened to restart.
    pub fn has_pending_interrupted_apply(&self) -> bool {
        self.rollback_path().exists()
    }

    /// Stages `new_binary_path` (an already downloaded and checksum-
    /// verified artifact) as the new active binary, moving the
    /// currently active one aside as `<binary>.rollback` rather than
    /// deleting it. Returns `Err(InterruptedApplyPending)` without
    /// touching anything if a previous apply's rollback file is still
    /// present — proceeding would risk losing the last known-good
    /// binary the caller may still need.
    pub fn stage_and_swap(&self, new_binary_path: &Path) -> Result<(), StagingError> {
        if self.has_pending_interrupted_apply() {
            return Err(StagingError::InterruptedApplyPending);
        }

        let staged_tmp = self.staged_tmp_path();
        std::fs::copy(new_binary_path, &staged_tmp)?;

        std::fs::rename(self.active_path(), self.rollback_path())?;
        std::fs::rename(&staged_tmp, self.active_path())?;
        Ok(())
    }

    /// Call after a successful health check — deletes the old binary
    /// that `stage_and_swap` kept around. Idempotent: calling it with
    /// no rollback file present (nothing pending) is a safe no-op, not
    /// an error — mirrors `lifecycle::CrashMarker::mark_clean_exit`'s
    /// established "absence is already the desired state" convention.
    pub fn commit(&self) -> Result<(), StagingError> {
        match std::fs::remove_file(self.rollback_path()) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Restores the previous binary after a failed health check (or an
    /// interrupted apply detected via `has_pending_interrupted_apply`).
    /// Safe even if `active_path` doesn't exist right now (the narrow
    /// window between `stage_and_swap`'s two renames) — removal is
    /// best-effort, the restoring rename is what actually matters.
    pub fn rollback(&self) -> Result<(), StagingError> {
        if !self.rollback_path().exists() {
            return Err(StagingError::NothingToRollBackTo);
        }
        std::fs::remove_file(self.active_path()).ok();
        std::fs::rename(self.rollback_path(), self.active_path())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "binary_name must be a plain file name")]
    fn new_rejects_a_binary_name_containing_a_path_separator() {
        Staging::new("/some/install/dir", "../escape/agent.bin");
    }

    #[test]
    #[should_panic(expected = "binary_name must be a plain file name")]
    fn new_rejects_a_bare_dot_dot_binary_name() {
        Staging::new("/some/install/dir", "..");
    }

    #[test]
    fn new_accepts_an_ordinary_plain_file_name() {
        let _ = Staging::new("/some/install/dir", "growth-layer-agent");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "updater-staging-test-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &[u8]) {
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn stage_and_swap_then_commit_leaves_the_new_binary_active_and_no_rollback_file() {
        let dir = temp_dir("commit");
        write(&dir.join("agent.bin"), b"old version bytes");
        let new_binary = dir.join("downloaded-new");
        write(&new_binary, b"new version bytes");

        let staging = Staging::new(&dir, "agent.bin");
        staging.stage_and_swap(&new_binary).unwrap();

        assert_eq!(
            std::fs::read(dir.join("agent.bin")).unwrap(),
            b"new version bytes"
        );
        assert!(dir.join("agent.bin.rollback").exists());
        assert!(staging.has_pending_interrupted_apply());

        staging.commit().unwrap();
        assert!(!dir.join("agent.bin.rollback").exists());
        assert!(!staging.has_pending_interrupted_apply());
        assert_eq!(
            std::fs::read(dir.join("agent.bin")).unwrap(),
            b"new version bytes"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stage_and_swap_then_rollback_restores_the_exact_old_binary() {
        let dir = temp_dir("rollback");
        write(&dir.join("agent.bin"), b"old version bytes, byte for byte");
        let new_binary = dir.join("downloaded-new");
        write(&new_binary, b"new version bytes that fail health check");

        let staging = Staging::new(&dir, "agent.bin");
        staging.stage_and_swap(&new_binary).unwrap();
        assert_eq!(
            std::fs::read(dir.join("agent.bin")).unwrap(),
            b"new version bytes that fail health check"
        );

        staging.rollback().unwrap();
        assert_eq!(
            std::fs::read(dir.join("agent.bin")).unwrap(),
            b"old version bytes, byte for byte"
        );
        assert!(!dir.join("agent.bin.rollback").exists());
        assert!(!staging.has_pending_interrupted_apply());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_interrupted_apply_is_detected_and_recoverable_via_rollback() {
        // Simulates a crash/power loss between stage_and_swap() and
        // commit()/rollback(): a fresh Staging instance (as a real
        // process restart would construct) must see the pending state
        // and be able to recover from it.
        let dir = temp_dir("interrupted");
        write(&dir.join("agent.bin"), b"old version");
        let new_binary = dir.join("downloaded-new");
        write(&new_binary, b"new version, health check never ran");

        {
            let staging = Staging::new(&dir, "agent.bin");
            staging.stage_and_swap(&new_binary).unwrap();
            // Process "crashes" here — neither commit() nor rollback() called.
        }

        let fresh_staging = Staging::new(&dir, "agent.bin");
        assert!(
            fresh_staging.has_pending_interrupted_apply(),
            "a fresh instance must detect the pending apply left by the 'crashed' one"
        );
        fresh_staging.rollback().unwrap();
        assert_eq!(
            std::fs::read(dir.join("agent.bin")).unwrap(),
            b"old version"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stage_and_swap_refuses_to_proceed_over_an_unresolved_pending_apply() {
        let dir = temp_dir("pending-refusal");
        write(&dir.join("agent.bin"), b"version A");
        let first_new = dir.join("downloaded-b");
        write(&first_new, b"version B");
        let second_new = dir.join("downloaded-c");
        write(&second_new, b"version C");

        let staging = Staging::new(&dir, "agent.bin");
        staging.stage_and_swap(&first_new).unwrap(); // now .rollback holds "version A", active is "version B"

        let result = staging.stage_and_swap(&second_new);
        assert!(matches!(result, Err(StagingError::InterruptedApplyPending)));
        // Must not have touched anything — version B is still active, version A still recoverable.
        assert_eq!(std::fs::read(dir.join("agent.bin")).unwrap(), b"version B");
        assert_eq!(
            std::fs::read(dir.join("agent.bin.rollback")).unwrap(),
            b"version A"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rollback_without_a_pending_apply_is_a_clear_error_not_silently_ignored() {
        let dir = temp_dir("rollback-nothing-pending");
        write(&dir.join("agent.bin"), b"version A");
        let staging = Staging::new(&dir, "agent.bin");
        assert!(matches!(
            staging.rollback(),
            Err(StagingError::NothingToRollBackTo)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_without_a_pending_apply_is_a_safe_noop() {
        let dir = temp_dir("commit-noop");
        write(&dir.join("agent.bin"), b"version A");
        let staging = Staging::new(&dir, "agent.bin");
        assert!(staging.commit().is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn queue_and_config_files_elsewhere_are_never_touched_by_staging() {
        // Proves "Queue survives"/"Pairing survives" structurally: this
        // crate only ever touches files inside install_dir named
        // exactly `binary_name`(.rollback/.staged) — a separate "data
        // dir" with queue/device_id files must be byte-identical
        // before and after a full stage->rollback cycle.
        let install_dir = temp_dir("scope-install");
        let data_dir = temp_dir("scope-data");
        write(&install_dir.join("agent.bin"), b"old binary");
        write(
            &data_dir.join("device_id.json"),
            b"{\"device_id\":\"abc-123\"}",
        );
        write(&data_dir.join("queue_record.json"), b"some queued event");

        let new_binary = install_dir.join("downloaded-new");
        write(&new_binary, b"new binary that will fail health check");

        let staging = Staging::new(&install_dir, "agent.bin");
        staging.stage_and_swap(&new_binary).unwrap();
        staging.rollback().unwrap();

        assert_eq!(
            std::fs::read(data_dir.join("device_id.json")).unwrap(),
            b"{\"device_id\":\"abc-123\"}"
        );
        assert_eq!(
            std::fs::read(data_dir.join("queue_record.json")).unwrap(),
            b"some queued event"
        );

        std::fs::remove_dir_all(&install_dir).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }
}
