//! `_NET_ACTIVE_WINDOW` (EWMH root-window property) → `_NET_WM_PID` —
//! confirmed the standard, current mechanism for X11 active-window/
//! process detection (freedesktop.org EWMH spec) during AG-LNX-001's
//! research, and already named in `agent/platforms/linux/collector.py`'s
//! own docstring. Resolves a PID only — the window's title is never
//! read here, matching this whole project's "never a title, always a
//! name" privacy invariant (see `windows_collector::browser_title`'s
//! doc comment for the same principle on the other platform).

use super::X11Error;
use x11rb::protocol::xproto::{self, AtomEnum};
use x11rb::rust_connection::RustConnection;

pub(super) fn intern(conn: &RustConnection, name: &[u8]) -> Result<u32, X11Error> {
    Ok(xproto::intern_atom(conn, false, name)?.reply()?.atom)
}

/// Reads a single 32-bit property value (a window ID or a PID — EWMH
/// represents both as `CARDINAL`/`WINDOW`, which are wire-identical
/// 32-bit values), or `None` if the property isn't set on this window
/// right now.
fn read_u32_property(
    conn: &RustConnection,
    window: u32,
    property: u32,
    type_: AtomEnum,
) -> Result<Option<u32>, X11Error> {
    let reply = xproto::get_property(conn, false, window, property, type_, 0, 1)?.reply()?;
    if reply.value.len() < 4 {
        return Ok(None);
    }
    Ok(Some(u32::from_ne_bytes(
        reply.value[0..4].try_into().unwrap(),
    )))
}

pub(super) fn active_window_pid(
    conn: &RustConnection,
    root: u32,
    net_active_window: u32,
    net_wm_pid: u32,
) -> Result<Option<u32>, X11Error> {
    let Some(active_window) = read_u32_property(conn, root, net_active_window, AtomEnum::WINDOW)?
    else {
        return Ok(None);
    };
    if active_window == 0 {
        return Ok(None); // EWMH: 0 means "no active window"
    }
    read_u32_property(conn, active_window, net_wm_pid, AtomEnum::CARDINAL)
}

#[cfg(test)]
mod tests {
    // `active_window_pid`/`intern` both require a real, live X11
    // connection — genuinely exercised in this crate's
    // `examples/collector_demo.rs` against the real WSLg X session (see
    // TEST_REPORT.md), not here: a unit test that can't open a display
    // would either be skipped (misleading — looks tested, isn't) or
    // require a display-detection guard that itself needs testing. The
    // one thing that IS pure here, `read_u32_property`'s 32-bit
    // little/native-endian decode, is exercised indirectly by every
    // live run — kept unduplicated rather than adding a synthetic-bytes
    // test for four lines of `from_ne_bytes`.
}
