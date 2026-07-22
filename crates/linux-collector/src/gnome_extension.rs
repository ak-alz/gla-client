//! Client side of a small companion GNOME Shell extension
//! (`installer/linux/gnome-extension/`) — the only way left to query
//! GNOME's focused window at all, since `org.gnome.Shell.Eval` has been
//! gated behind "unsafe mode" (off by default) since GNOME 41 (see
//! `environment.rs`'s doc comment and
//! `docs/02_ARCHITECTURE/AGENT_LINUX_CAPABILITY_MATRIX.md`, which
//! flagged this exact extension as the single largest open risk this
//! crate inherited — no longer unimplemented as of this module).
//!
//! The extension exports `org.growthlayer.AgentHelper.GetFocusedWindow`
//! under GNOME Shell's OWN well-known bus name (`org.gnome.Shell`) —
//! Shell extensions run inside the Shell process itself and export
//! objects on the connection it already owns, so there is no separate
//! service name to register or race to acquire.
//!
//! # What could and could not be verified for real in this environment
//!
//! This crate's dev/verification environment (WSLg) has no GNOME Shell
//! at all (WSLg runs a minimal Weston compositor, not a full desktop
//! session) — there is no way to load the extension or exercise this
//! module against a real Shell process here. The D-Bus call plumbing
//! below is the same, already-verified `zbus::blocking::Connection`
//! pattern `native_loop.rs` uses against real logind — but the
//! extension side of the contract (does `global.display.focus_window`
//! really return what's expected, does the GJS D-Bus export actually
//! work end to end) is genuinely unverified until run on a real GNOME
//! session, exactly like `macos-collector`'s implementations were
//! written blind earlier this project. Treat this as code-complete, not
//! field-verified, until confirmed on a real machine.

use thiserror::Error;
use zbus::blocking::Connection;

const SERVICE: &str = "org.gnome.Shell";
const OBJECT_PATH: &str = "/org/growthlayer/AgentHelper";
const INTERFACE: &str = "org.growthlayer.AgentHelper";
const METHOD: &str = "GetFocusedWindow";

#[derive(Debug, Error)]
pub enum GnomeExtensionError {
    #[error("failed to connect to the D-Bus session bus: {0}")]
    Connect(zbus::Error),
    /// Covers every real-world "not available" case identically (not
    /// installed, not enabled, installed but not yet loaded since that
    /// needs a fresh login on Wayland — see the extension's own
    /// metadata) — there is no portable way to distinguish these from
    /// the D-Bus error alone, and the caller's fallback (report
    /// unavailable, never guess) is the same for all of them anyway.
    #[error("the growth-layer-agent GNOME Shell extension did not respond (not installed, not enabled, or not loaded yet — a fresh install needs a log out/in): {0}")]
    CallFailed(zbus::Error),
}

/// One D-Bus connection, reused across polls — matches
/// `x11::X11Session`'s own persistent-connection shape, and avoids
/// hiding a genuinely broken extension behind "well some poll
/// eventually reconnects".
pub struct GnomeExtensionSession {
    conn: Connection,
}

impl GnomeExtensionSession {
    pub fn connect() -> Result<Self, GnomeExtensionError> {
        let conn = Connection::session().map_err(GnomeExtensionError::Connect)?;
        Ok(GnomeExtensionSession { conn })
    }

    /// `Ok(None)` means the extension responded but no window is
    /// focused (e.g. an empty workspace) — mirrors
    /// `x11::X11Session::active_window_pid`'s own `Option` semantics,
    /// not an error. Any D-Bus failure (extension absent/disabled/not
    /// yet loaded) is `Err`, surfaced by the caller as the same
    /// `UnsupportedReason::GnomeRequiresShellExtension` the pre-
    /// extension code already reports — never fabricated as "no window
    /// focused".
    pub fn focused_window_pid(&self) -> Result<Option<u32>, GnomeExtensionError> {
        let reply = self
            .conn
            .call_method(Some(SERVICE), OBJECT_PATH, Some(INTERFACE), METHOD, &())
            .map_err(GnomeExtensionError::CallFailed)?;
        let (wm_class, pid): (String, i32) = reply
            .body()
            .deserialize()
            .map_err(GnomeExtensionError::CallFailed)?;
        if wm_class.is_empty() {
            return Ok(None);
        }
        Ok(u32::try_from(pid).ok())
    }
}
