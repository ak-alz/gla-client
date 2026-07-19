//! Minimal cross-platform tray status/diagnostics shell (AG-007). Deliberately
//! thin, per `CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md`'s explicit
//! item list: agent status, paired/unpaired, last sync, pause/resume,
//! diagnostics, open dashboard, check for updates, version, quit,
//! uninstall/help link — nothing else. No charts, no embedded dashboard,
//! no settings beyond pause/resume, no analytics, no marketing screens
//! (see that file's "Не добавлять" list) — `menu_model.rs`'s `MenuAction`
//! enum structurally cannot represent any of those.
//!
//! `status.rs` and `menu_model.rs` are pure logic, fully unit-tested
//! without any OS dependency. `tray.rs` is the only module touching real
//! native APIs (tray-icon + winit) and is verified by actually running it
//! (see TEST_REPORT.md), not by automated tests — there is no meaningful
//! way to assert "a real system tray icon appeared" in a headless run.
//!
//! Per ADR 0013 (binding): this crate never constructs a window of any
//! kind. "Open dashboard"/"Diagnostics" open the system's default browser
//! (`browser.rs`) instead of an embedded Tauri webview — a deliberate,
//! more conservative choice than what ADR 0013 permitted (see
//! `browser.rs`'s doc comment for the full reasoning), not an oversight.

mod browser;
mod menu_model;
mod status;
mod tray;

pub use browser::{open_url, OpenUrlError};
pub use menu_model::{build_menu, MenuAction, MenuEntry};
pub use status::{
    last_sync_line, pause_resume_label, pending_line, status_line, version_line, AgentStatus,
};
#[cfg(target_os = "linux")]
pub use tray::request_quit;
pub use tray::{run_tray, AgentController};
