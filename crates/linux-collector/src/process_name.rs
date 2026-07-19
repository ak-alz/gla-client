//! `/proc/<pid>/comm` — the standard, kernel-provided way to resolve a
//! PID to a process (command) name on Linux, matching `psutil.Process
//! (pid).name()`'s output on the Python source (`platforms/windows/
//! collector.py`'s Windows equivalent already documents the identical
//! "name, never a path or title" contract this mirrors).

pub fn process_name_for_pid(pid: u32) -> Option<String> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = raw.trim_end_matches('\n');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_this_test_processs_own_pid() {
        // A real, live PID this test process itself owns — genuinely
        // exercises the real /proc filesystem read, not a fixture.
        let pid = std::process::id();
        let name = process_name_for_pid(pid);
        assert!(
            name.is_some(),
            "must resolve our own running process's name from /proc/{pid}/comm"
        );
    }

    #[test]
    fn nonexistent_pid_returns_none() {
        // PID 1 is always init/systemd, never this test binary; a PID
        // far beyond any real process (and beyond a fresh boot's PID
        // counter) is a safe, portable "doesn't exist" probe.
        assert_eq!(process_name_for_pid(u32::MAX - 1), None);
    }
}
