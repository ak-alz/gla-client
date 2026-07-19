//! X11 backend — the "полноценно в MVP" path per
//! `agent/platforms/linux/collector.py`'s own docstring, and the only
//! backend with a mature, single, cross-desktop-environment API (EWMH),
//! unlike Wayland's per-compositor fragmentation (see
//! AGENT_LINUX_CAPABILITY_MATRIX.md).
//!
//! One `X11Session` owns a single, real, live connection for the whole
//! life of the collector (opened once at `start()`, not reconnected
//! every `poll()`) with the two EWMH atoms it needs already interned —
//! matching the same "connect once, reuse" shape as
//! `windows_collector`'s Win32 API usage, which has no separate
//! connect/reconnect step to mirror in the first place.

mod active_window;
mod idle;

use thiserror::Error;
use x11rb::connection::Connection as _;
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Error)]
pub enum X11Error {
    #[error("failed to connect to the X server: {0}")]
    Connect(#[from] x11rb::errors::ConnectError),
    #[error("X11 request failed: {0}")]
    Connection(#[from] x11rb::errors::ConnectionError),
    #[error("X11 reply error: {0}")]
    Reply(#[from] x11rb::errors::ReplyError),
}

pub struct X11Session {
    conn: RustConnection,
    root: u32,
    net_active_window: u32,
    net_wm_pid: u32,
}

impl X11Session {
    pub fn connect() -> Result<Self, X11Error> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let net_active_window = active_window::intern(&conn, b"_NET_ACTIVE_WINDOW")?;
        let net_wm_pid = active_window::intern(&conn, b"_NET_WM_PID")?;
        Ok(X11Session {
            conn,
            root,
            net_active_window,
            net_wm_pid,
        })
    }

    /// The PID of the process owning the currently active window, or
    /// `None` if there is no active window right now (nothing
    /// pathological — e.g. a brief moment between window switches).
    pub fn active_window_pid(&self) -> Result<Option<u32>, X11Error> {
        active_window::active_window_pid(
            &self.conn,
            self.root,
            self.net_active_window,
            self.net_wm_pid,
        )
    }

    pub fn idle_seconds(&self) -> Result<f64, X11Error> {
        idle::idle_seconds(&self.conn, self.root)
    }
}
