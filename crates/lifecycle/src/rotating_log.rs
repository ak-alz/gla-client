//! A minimal rotating log for lifecycle EVENTS only (start/stop/crash-
//! detected/suspend/resume/lock/unlock) — never raw payload or credentials,
//! matching the same "no raw payload/token in logs" discipline already
//! established in `uploader` (AG-005). Numeric rotation
//! (`lifecycle.log` → `lifecycle.log.1` → `lifecycle.log.2` → ... →
//! deleted past `max_rotated_files`), the same scheme most Unix logrotate
//! configurations default to.

use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct RotatingLog {
    dir: PathBuf,
    base_name: String,
    max_bytes: u64,
    max_rotated_files: usize,
}

impl RotatingLog {
    pub fn new(
        dir: PathBuf,
        base_name: impl Into<String>,
        max_bytes: u64,
        max_rotated_files: usize,
    ) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(RotatingLog {
            dir,
            base_name: base_name.into(),
            max_bytes,
            max_rotated_files,
        })
    }

    fn current_path(&self) -> PathBuf {
        self.dir.join(&self.base_name)
    }

    fn rotated_path(&self, n: usize) -> PathBuf {
        self.dir.join(format!("{}.{n}", self.base_name))
    }

    fn rotate(&self) -> std::io::Result<()> {
        let oldest = self.rotated_path(self.max_rotated_files);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for n in (1..self.max_rotated_files).rev() {
            let from = self.rotated_path(n);
            if from.exists() {
                fs::rename(&from, self.rotated_path(n + 1))?;
            }
        }
        let current = self.current_path();
        if current.exists() {
            fs::rename(&current, self.rotated_path(1))?;
        }
        Ok(())
    }

    /// Appends one timestamped line, rotating first if the current file is
    /// already at or past `max_bytes`. Rotating BEFORE writing (not after)
    /// means the current file never grows unboundedly past `max_bytes`
    /// between checks — the check happens on every single append, not
    /// periodically.
    pub fn append(&self, message: &str) -> std::io::Result<()> {
        let current = self.current_path();
        let current_size = fs::metadata(&current).map(|m| m.len()).unwrap_or(0);
        if self.max_rotated_files > 0 && current_size >= self.max_bytes {
            self.rotate()?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current)?;
        writeln!(file, "{} {message}", Utc::now().to_rfc3339())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lifecycle-log-test-{label}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn appended_lines_are_readable_and_timestamped() {
        let dir = temp_dir("basic");
        let log = RotatingLog::new(dir.clone(), "test.log", 1_000_000, 3).unwrap();
        log.append("agent started").unwrap();
        log.append("agent paused (suspend)").unwrap();

        let content = fs::read_to_string(dir.join("test.log")).unwrap();
        assert!(content.contains("agent started"));
        assert!(content.contains("agent paused (suspend)"));
        // A real RFC3339 timestamp prefix, not just the raw message.
        assert!(
            content.lines().next().unwrap().starts_with("20"),
            "expected a timestamp prefix, got: {:?}",
            content.lines().next()
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rotates_when_max_bytes_exceeded_and_caps_rotated_file_count() {
        let dir = temp_dir("rotation");
        // Tiny limit so a handful of short lines forces multiple rotations.
        let log = RotatingLog::new(dir.clone(), "test.log", 20, 2).unwrap();
        for i in 0..30 {
            log.append(&format!("line {i}")).unwrap();
        }

        assert!(
            dir.join("test.log").exists(),
            "current log file must always exist after at least one append"
        );
        assert!(dir.join("test.log.1").exists());
        assert!(dir.join("test.log.2").exists());
        assert!(
            !dir.join("test.log.3").exists(),
            "must never keep more than max_rotated_files old files"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_data_is_lost_across_a_single_rotation_boundary() {
        let dir = temp_dir("no-loss");
        let log = RotatingLog::new(dir.clone(), "test.log", 15, 5).unwrap();
        log.append("first").unwrap();
        log.append("second").unwrap(); // likely forces the first rotation

        let current = fs::read_to_string(dir.join("test.log")).unwrap_or_default();
        let rotated = fs::read_to_string(dir.join("test.log.1")).unwrap_or_default();
        let combined = format!("{rotated}{current}");
        assert!(combined.contains("first"));
        assert!(combined.contains("second"));

        fs::remove_dir_all(&dir).ok();
    }
}
