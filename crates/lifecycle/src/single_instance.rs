//! Single-instance enforcement via an OS-level advisory file lock — the
//! exact same primitive already proven load-bearing in
//! `durable-queue::DurableQueue`'s directory lock (AG-004): `File::try_lock`
//! is held for the lifetime of the guard and released automatically on
//! `Drop` or by the OS if the process dies without a clean shutdown, so a
//! crashed previous instance can never permanently block a new one from
//! starting — no stale-lock-cleanup logic is needed, by construction.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SingleInstanceError {
    #[error("another instance is already running (lock file: {lock_path:?})")]
    AlreadyRunning { lock_path: PathBuf },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Holding this alive is what enforces single-instance; dropping it (or
/// the process exiting, however it exits) releases the lock.
pub struct SingleInstanceGuard {
    _lock_file: File,
}

/// Attempts to become the sole instance holding `lock_path`. Returns
/// `Err(AlreadyRunning)` if another live process already holds it —
/// callers should exit promptly in that case, not retry or wait (the
/// "Нет duplicate processes" acceptance criterion is about never running
/// two at once, not about queueing a second launch attempt).
pub fn acquire(lock_path: &Path) -> Result<SingleInstanceGuard, SingleInstanceError> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    match lock_file.try_lock() {
        Ok(()) => Ok(SingleInstanceGuard {
            _lock_file: lock_file,
        }),
        Err(std::fs::TryLockError::WouldBlock) => Err(SingleInstanceError::AlreadyRunning {
            lock_path: lock_path.to_path_buf(),
        }),
        Err(std::fs::TryLockError::Error(err)) => Err(SingleInstanceError::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid_lite::new_id;

    mod uuid_lite {
        // A tiny, dependency-free unique-id generator for test temp paths —
        // this crate has no reason to depend on the `uuid` crate itself for
        // production code, only tests need uniqueness here.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        pub fn new_id() -> u64 {
            COUNTER.fetch_add(1, Ordering::Relaxed)
        }
    }

    fn temp_lock_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lifecycle-single-instance-test-{label}-{}.lock",
            new_id()
        ))
    }

    #[test]
    fn first_acquire_succeeds_second_fails_while_first_is_held() {
        let path = temp_lock_path("basic");
        let first = acquire(&path).expect("first acquire must succeed");
        let second = acquire(&path);
        assert!(
            matches!(second, Err(SingleInstanceError::AlreadyRunning { .. })),
            "a second concurrent acquire must fail while the first is still held"
        );

        drop(first);
        let third = acquire(&path);
        assert!(third.is_ok(), "after the first guard is dropped, a new acquire must succeed — this is a real restart, not a conflict");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dropping_the_guard_releases_the_lock_for_a_subsequent_acquire() {
        let path = temp_lock_path("drop-release");
        {
            let _guard = acquire(&path).unwrap();
            assert!(acquire(&path).is_err());
        }
        assert!(
            acquire(&path).is_ok(),
            "lock must be released once the guard goes out of scope"
        );

        std::fs::remove_file(&path).ok();
    }
}
