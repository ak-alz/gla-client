//! XScreenSaver extension (`XScreenSaverQueryInfo`) — the standard X11
//! system idle-timer API, confirmed during AG-LNX-001's research and
//! already named in the Python docstring this crate ports. Mirrors
//! `windows_collector::idle`'s role exactly, just backed by a different
//! OS mechanism.

use super::X11Error;
use x11rb::protocol::screensaver;
use x11rb::rust_connection::RustConnection;

pub(super) fn idle_seconds(conn: &RustConnection, root: u32) -> Result<f64, X11Error> {
    let info = screensaver::query_info(conn, root)?.reply()?;
    Ok(info.ms_since_user_input as f64 / 1000.0)
}
