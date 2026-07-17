//! "Start on login" — Windows-only for now (this project's primary,
//! currently-only-testable platform; see `docs/05_ENGINEERING/ADR/0013-
//! cross-platform-agent-stack.md`). Linux/macOS autostart mechanisms
//! (XDG autostart `.desktop` files / macOS `launchd` agents) are a
//! documented gap, not silently assumed solved — see TEST_REPORT.md.

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

#[cfg(not(windows))]
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
