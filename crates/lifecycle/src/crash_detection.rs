//! Distinguishes a clean Quit from a crash across restarts (the "Quit
//! отличим от crash" acceptance criterion) via a marker file: written at
//! startup, removed only on a confirmed clean exit. If the marker is
//! already present at the NEXT startup, the previous run never reached
//! its clean-exit path — it crashed, was killed, or the machine lost
//! power — there is no other way that file could still exist.

use std::fs;
use std::path::PathBuf;

pub struct CrashMarker {
    path: PathBuf,
}

impl CrashMarker {
    pub fn new(path: PathBuf) -> Self {
        CrashMarker { path }
    }

    /// Call once at startup, BEFORE `mark_running()` — answers "did the
    /// previous run crash?" If this is the very first run ever (no marker
    /// file exists at all, not even from cleanup), this correctly returns
    /// `false`: there is no "previous run" to have crashed.
    pub fn previous_run_crashed(&self) -> bool {
        self.path.exists()
    }

    /// Call once at startup, right after checking `previous_run_crashed()`
    /// — marks THIS run as "in progress, not yet cleanly exited."
    pub fn mark_running(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, std::process::id().to_string())
    }

    /// Call on a confirmed clean shutdown path (the tray's Quit action, or
    /// a handled `EndSession` lifecycle event) — removes the marker,
    /// proving to the NEXT startup that this run exited on purpose.
    /// Idempotent: calling it when there is no marker (e.g. `mark_running`
    /// was never called, or this is called twice) is not an error.
    pub fn mark_clean_exit(&self) -> std::io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_marker_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lifecycle-crash-marker-test-{label}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn first_ever_run_is_not_reported_as_a_crash() {
        let path = temp_marker_path("first-run");
        let marker = CrashMarker::new(path);
        assert!(
            !marker.previous_run_crashed(),
            "no marker file at all means there is no previous run to have crashed"
        );
    }

    #[test]
    fn a_run_that_never_reaches_clean_exit_is_detected_as_crashed_on_the_next_startup() {
        let path = temp_marker_path("crash-detected");

        // Simulates run #1: starts, marks itself running, then "crashes"
        // (the process just stops here — mark_clean_exit() is never called).
        {
            let marker = CrashMarker::new(path.clone());
            assert!(!marker.previous_run_crashed());
            marker.mark_running().unwrap();
        }

        // Simulates run #2 (the "restart"): a brand-new CrashMarker
        // instance pointed at the same path, as a real process restart
        // would construct.
        let marker2 = CrashMarker::new(path.clone());
        assert!(
            marker2.previous_run_crashed(),
            "the marker left behind by the crashed run #1 must be detected by run #2"
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_run_that_exits_cleanly_is_not_detected_as_crashed_on_the_next_startup() {
        let path = temp_marker_path("clean-exit");

        {
            let marker = CrashMarker::new(path.clone());
            marker.mark_running().unwrap();
            marker.mark_clean_exit().unwrap(); // the whole point of this test
        }

        let marker2 = CrashMarker::new(path.clone());
        assert!(
            !marker2.previous_run_crashed(),
            "a clean exit must leave no trace for the next startup to misdetect as a crash"
        );
    }

    #[test]
    fn mark_clean_exit_without_a_prior_mark_running_is_a_safe_noop() {
        let path = temp_marker_path("noop");
        let marker = CrashMarker::new(path);
        assert!(marker.mark_clean_exit().is_ok());
    }
}
