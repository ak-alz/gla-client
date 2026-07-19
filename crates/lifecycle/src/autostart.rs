//! "Start on login" — Windows (registry Run key) and Linux (systemd user
//! service, per AG-LNX-002 — confirmed DE-agnostic and the more portable
//! choice over XDG autostart `.desktop` files during AG-LNX-001's
//! research; the two mechanisms compose via `systemd-xdg-autostart-
//! generator` rather than conflict, so shipping a real unit here doesn't
//! preclude also shipping a `.desktop` file later). macOS (`launchd`
//! agents) remains a documented gap — see TEST_REPORT.md.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AutostartError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("autostart is not implemented on this platform yet")]
    UnsupportedPlatform,
}

/// Manages one registered autostart entry, identified by `app_name`
/// (the registry value name on Windows), pointing at `executable_path`.
pub struct Autostart {
    app_name: String,
    executable_path: PathBuf,
}

impl Autostart {
    pub fn new(app_name: impl Into<String>, executable_path: PathBuf) -> Self {
        Autostart {
            app_name: app_name.into(),
            executable_path,
        }
    }

    /// The exact string this crate would write to (or compare against in)
    /// the registry — a quoted path, matching the Windows convention for
    /// Run-key command lines whose path may contain spaces.
    fn command_line(&self) -> String {
        format!("\"{}\"", self.executable_path.display())
    }

    pub fn is_enabled(&self) -> Result<bool, AutostartError> {
        imp::is_enabled(&self.app_name, &self.command_line())
    }

    /// Idempotent: enabling an already-enabled entry just rewrites the
    /// same value, no error.
    pub fn enable(&self) -> Result<(), AutostartError> {
        imp::enable(&self.app_name, &self.command_line())
    }

    /// Idempotent: disabling an entry that was never enabled (or already
    /// disabled) is a no-op, not an error — mirrors the same "undoing an
    /// already-undone action is safe" convention established in
    /// `durable-queue`'s `ack`/`release` and `uploader`'s backoff handling.
    /// This is also what "Uninstall удаляет autostart" needs: an
    /// uninstaller calling `disable()` must succeed even if the user had
    /// already turned autostart off themselves.
    pub fn disable(&self) -> Result<(), AutostartError> {
        imp::disable(&self.app_name)
    }
}

#[cfg(windows)]
mod imp {
    use super::AutostartError;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn run_key() -> std::io::Result<RegKey> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey_with_flags(RUN_KEY_PATH, KEY_READ | KEY_WRITE)
    }

    pub fn is_enabled(app_name: &str, expected_command_line: &str) -> Result<bool, AutostartError> {
        let key = run_key()?;
        match key.get_value::<String, _>(app_name) {
            Ok(value) => Ok(value == expected_command_line),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    pub fn enable(app_name: &str, command_line: &str) -> Result<(), AutostartError> {
        let key = run_key()?;
        key.set_value(app_name, &command_line)?;
        Ok(())
    }

    pub fn disable(app_name: &str) -> Result<(), AutostartError> {
        let key = run_key()?;
        match key.delete_value(app_name) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::AutostartError;
    use std::io;
    use std::path::PathBuf;
    use std::process::Command;

    /// `~/.config/systemd/user/` — the standard per-user systemd unit
    /// directory (`systemd.io/DESKTOP_ENVIRONMENTS`), confirmed during
    /// AG-LNX-001's research as the more portable pattern than XDG
    /// autostart `.desktop` files for a background agent specifically.
    fn unit_dir() -> Result<PathBuf, AutostartError> {
        let home = std::env::var_os("HOME").ok_or(AutostartError::UnsupportedPlatform)?;
        Ok(PathBuf::from(home).join(".config/systemd/user"))
    }

    fn unit_path(app_name: &str) -> Result<PathBuf, AutostartError> {
        Ok(unit_dir()?.join(format!("{app_name}.service")))
    }

    /// Deliberately minimal (`Type=simple`, no restart policy, no
    /// resource limits) — this crate's job is registering the unit, not
    /// authoring a production-grade one; a future task can extend this
    /// template without changing the `enable`/`disable`/`is_enabled`
    /// contract.
    fn unit_contents(command_line: &str) -> String {
        format!(
            "[Unit]\nDescription={command_line} (autostart)\n\n\
             [Service]\nExecStart={command_line}\nType=simple\n\n\
             [Install]\nWantedBy=default.target\n"
        )
    }

    fn run_systemctl(args: &[&str]) -> Result<(), AutostartError> {
        let status = Command::new("systemctl").args(args).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(AutostartError::Io(io::Error::other(format!(
                "systemctl {args:?} exited with {status}"
            ))))
        }
    }

    pub fn is_enabled(app_name: &str, expected_command_line: &str) -> Result<bool, AutostartError> {
        let path = unit_path(app_name)?;
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Ok(false);
        };
        if contents != unit_contents(expected_command_line) {
            // A unit exists under this name but doesn't match what this
            // executable would write (stale path, hand-edited, or a
            // different app entirely) — not "enabled" in the sense this
            // caller means, matching the Windows path's exact-string
            // comparison against the registry value.
            return Ok(false);
        }
        let output = Command::new("systemctl")
            .args(["--user", "is-enabled", &format!("{app_name}.service")])
            .output()?;
        Ok(output.status.success())
    }

    /// Idempotent: re-running `enable()` rewrites the same unit file and
    /// re-runs `systemctl --user enable`, which systemd itself already
    /// treats as a safe no-op on an already-enabled unit.
    pub fn enable(app_name: &str, command_line: &str) -> Result<(), AutostartError> {
        let dir = unit_dir()?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(unit_path(app_name)?, unit_contents(command_line))?;
        run_systemctl(&["--user", "daemon-reload"])?;
        run_systemctl(&["--user", "enable", &format!("{app_name}.service")])
    }

    /// Idempotent: disabling a never-enabled unit must never fail —
    /// unlike `enable`, where `systemctl --user enable` on an
    /// already-enabled unit really is documented as a safe no-op,
    /// `systemctl --user disable` on a unit systemd has never heard of
    /// (no unit file, never enabled) exits non-zero ("Unit ... does not
    /// exist") — a real, confirmed-by-testing difference from the
    /// initial assumption that `disable` would be uniformly safe like
    /// its Windows counterpart. The exit code is intentionally ignored
    /// here (not routed through `run_systemctl`, which does check it) —
    /// this call's only job is "make sure it's off," and a unit that was
    /// never on is already exactly that.
    pub fn disable(app_name: &str) -> Result<(), AutostartError> {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", &format!("{app_name}.service")])
            .status();
        let path = unit_path(app_name)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        run_systemctl(&["--user", "daemon-reload"])
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::AutostartError;

    pub fn is_enabled(
        _app_name: &str,
        _expected_command_line: &str,
    ) -> Result<bool, AutostartError> {
        Err(AutostartError::UnsupportedPlatform)
    }

    pub fn enable(_app_name: &str, _command_line: &str) -> Result<(), AutostartError> {
        Err(AutostartError::UnsupportedPlatform)
    }

    pub fn disable(_app_name: &str) -> Result<(), AutostartError> {
        Err(AutostartError::UnsupportedPlatform)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    /// Same reasoning as the Windows test module's `test_app_name`:
    /// `cargo test` runs tests as threads within one process, so a name
    /// varying only by `std::process::id()` would race across tests in
    /// this module against the same real unit file/systemd state.
    fn test_app_name() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!(
            "growth-layer-autostart-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn disabled_by_default_then_enable_is_reflected_then_disable_removes_it() {
        let app_name = test_app_name();
        let autostart = Autostart::new(app_name, PathBuf::from("/fake/path/growth-layer-agent"));

        assert!(
            !autostart.is_enabled().unwrap(),
            "must not already be enabled under a fresh, unique test name"
        );

        autostart.enable().unwrap();
        assert!(
            autostart.is_enabled().unwrap(),
            "must be enabled immediately after enable()"
        );

        autostart.disable().unwrap();
        assert!(
            !autostart.is_enabled().unwrap(),
            "must be disabled immediately after disable()"
        );
    }

    #[test]
    fn disable_on_a_never_enabled_entry_is_a_safe_noop() {
        let app_name = test_app_name();
        let autostart = Autostart::new(app_name, PathBuf::from("/fake/path/growth-layer-agent"));
        assert!(
            autostart.disable().is_ok(),
            "disabling something never enabled must not error"
        );
    }

    #[test]
    fn enable_is_idempotent() {
        let app_name = test_app_name();
        let autostart = Autostart::new(app_name, PathBuf::from("/fake/path/growth-layer-agent"));
        autostart.enable().unwrap();
        autostart.enable().unwrap(); // must not error on a second call
        assert!(autostart.is_enabled().unwrap());
        autostart.disable().unwrap();
    }

    #[test]
    fn a_unit_belonging_to_a_different_executable_path_is_not_considered_enabled() {
        let app_name = test_app_name();
        let a = Autostart::new(app_name.clone(), PathBuf::from("/fake/path/one"));
        let b = Autostart::new(app_name, PathBuf::from("/fake/path/two"));
        a.enable().unwrap();
        assert!(a.is_enabled().unwrap());
        assert!(
            !b.is_enabled().unwrap(),
            "same app_name but a different executable path must not read as enabled"
        );
        a.disable().unwrap();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A distinctive, throwaway registry value name, unique per call (not
    /// just per process): `cargo test` runs tests concurrently as threads
    /// within ONE process, so a name that only varied by
    /// `std::process::id()` would be identical across every test in this
    /// module, racing on the exact same real registry value — this bit a
    /// first version of this test file (an intermittent, environment-
    /// dependent failure, not a bug in `Autostart` itself; see
    /// TEST_REPORT.md). Also never reused by the real agent's actual
    /// autostart entry (which would use a stable, product-specific name),
    /// so this test can never collide with or disturb a real installation.
    fn test_app_name() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!(
            "GrowthLayerAutostartTest-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn disabled_by_default_then_enable_is_reflected_then_disable_removes_it() {
        let app_name = test_app_name();
        let autostart = Autostart::new(
            app_name,
            PathBuf::from(r"C:\fake\path\growth-layer-agent.exe"),
        );

        assert!(
            !autostart.is_enabled().unwrap(),
            "must not already be enabled under a fresh, unique test name"
        );

        autostart.enable().unwrap();
        assert!(
            autostart.is_enabled().unwrap(),
            "must be enabled immediately after enable()"
        );

        autostart.disable().unwrap();
        assert!(
            !autostart.is_enabled().unwrap(),
            "must be disabled immediately after disable()"
        );
    }

    #[test]
    fn disable_on_a_never_enabled_entry_is_a_safe_noop() {
        let app_name = test_app_name();
        let autostart = Autostart::new(
            app_name,
            PathBuf::from(r"C:\fake\path\growth-layer-agent.exe"),
        );
        assert!(autostart.disable().is_ok(), "disabling something never enabled must not error — needed for uninstall to be safe regardless of prior state");
    }

    #[test]
    fn enable_is_idempotent() {
        let app_name = test_app_name();
        let autostart = Autostart::new(
            app_name,
            PathBuf::from(r"C:\fake\path\growth-layer-agent.exe"),
        );
        autostart.enable().unwrap();
        autostart.enable().unwrap(); // must not error on a second call
        assert!(autostart.is_enabled().unwrap());
        autostart.disable().unwrap();
    }
}
