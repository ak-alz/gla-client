//! Free disk space check before starting a download — "free disk
//! check" per the task's own bullet list. `fs4` (a maintained fork of
//! the older, now-unmaintained `fs2`) gives a real, cross-platform
//! (Windows `GetDiskFreeSpaceExW` / POSIX `statvfs`) available-space
//! query — not a heavier system-info crate pulling in stats this
//! crate doesn't need, consistent with ADR 0013's minimal-footprint
//! goal.

use fs4::available_space;
use std::path::Path;

/// `true` if at least `required_bytes` are free on the filesystem
/// containing `path`. `path` should be a real, existing directory
/// (e.g. the install dir the download/staging will actually use) —
/// querying the wrong filesystem's free space would defeat the point
/// of this check.
pub fn has_enough_free_space(path: &Path, required_bytes: u64) -> std::io::Result<bool> {
    Ok(available_space(path)? >= required_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_temp_dir_reports_some_nonzero_free_space() {
        // A real, live query against a real filesystem this machine
        // actually has — not mocked. Any reasonable dev/CI machine has
        // at least 1 byte free; this just confirms the underlying
        // syscall path genuinely works end to end.
        let tmp = std::env::temp_dir();
        assert!(has_enough_free_space(&tmp, 1).unwrap());
    }

    #[test]
    fn an_unreasonably_large_requirement_is_reported_as_insufficient() {
        let tmp = std::env::temp_dir();
        let absurd = u64::MAX / 2;
        assert!(!has_enough_free_space(&tmp, absurd).unwrap());
    }
}
