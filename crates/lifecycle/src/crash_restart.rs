//! Registers this process with Windows so that Windows Error Reporting
//! (or an update/reboot in progress) relaunches it automatically after an
//! unexpected termination — the OS-native mechanism for "crash restart" on
//! Windows (`RegisterApplicationRestart`), rather than a hand-rolled
//! watchdog process, which would itself need its own lifecycle management
//! (who restarts the watchdog?).
//!
//! What this crate can verify automatically: that the registration CALL
//! itself succeeds (a real, observable `HRESULT`). What it cannot verify
//! automatically: that Windows Error Reporting actually relaunches the
//! process after a real, uncontrolled crash — WER's behavior depends on
//! machine-wide settings (Group Policy, "Problem Reports and Solutions"
//! configuration, whether a debugger is attached) that this crate does not
//! control and that aren't safe to rely on being in a specific state for
//! an automated test. See TEST_REPORT.md for how this was handled.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RestartError {
    #[error("RegisterApplicationRestart failed with HRESULT {0:#010x}")]
    Failed(i32),
    #[error("not implemented on this platform")]
    UnsupportedPlatform,
}

/// Registers this process for automatic restart. `command_line_args` is
/// what Windows will pass back to the relaunched process (kept short and
/// non-sensitive — see the module doc comment on why raw state never
/// belongs here, matching `uploader`'s "no raw payload in logs" discipline
/// applied to a different channel).
#[cfg(windows)]
pub fn register_for_crash_restart(command_line_args: &str) -> Result<(), RestartError> {
    use windows_sys::Win32::Foundation::S_OK;
    use windows_sys::Win32::System::Recovery::RegisterApplicationRestart;

    let wide: Vec<u16> = command_line_args
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // Flags = 0: restart after all the default-covered termination
    // reasons (crash, hang, and — separately from a bare crash but still
    // relevant to "does the agent come back on its own" — a pending
    // update install or an OS-initiated reboot). Passing any of the
    // RESTART_NO_* flags would OPT OUT of restarting for that specific
    // reason, which is the opposite of this task's goal.
    let hr = unsafe { RegisterApplicationRestart(wide.as_ptr(), 0) };
    if hr == S_OK {
        Ok(())
    } else {
        Err(RestartError::Failed(hr))
    }
}

#[cfg(not(windows))]
pub fn register_for_crash_restart(_command_line_args: &str) -> Result<(), RestartError> {
    Err(RestartError::UnsupportedPlatform)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn registration_call_succeeds() {
        // This only proves the OS accepted the registration (a real,
        // observable HRESULT) — it does NOT prove WER will actually
        // relaunch this process after a real crash, which this crate
        // cannot control or safely simulate in an automated test (see
        // module doc comment and TEST_REPORT.md).
        let result = register_for_crash_restart("--restarted-after-crash");
        assert!(
            result.is_ok(),
            "expected RegisterApplicationRestart to succeed, got: {result:?}"
        );
    }
}
