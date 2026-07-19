//! Linux platform signal collector (AG-LNX-002): real X11 (EWMH active
//! window, XScreenSaver idle) and Hyprland (IPC active window) backends,
//! a shared `evdev`-based input-event counter (also the idle-time
//! fallback for Hyprland/Wayland, see `input_counters.rs`), real
//! `org.freedesktop.login1` (logind) D-Bus session/power-event
//! registration feeding the same `lifecycle::PowerEvent` the Windows
//! backend uses, and systemd-user-service autostart (implemented in
//! `lifecycle::Autostart`'s Linux path, not duplicated here).
//!
//! GNOME and KDE (Wayland) explicitly report
//! [`environment::UnsupportedReason`] rather than guessing or silently
//! returning stale data — see `docs/02_ARCHITECTURE/
//! AGENT_LINUX_CAPABILITY_MATRIX.md` for why (GNOME requires a
//! not-yet-shipped Shell extension, KDE a not-yet-shipped KWin script)
//! and the user's explicit scoping decision for this task.
//!
//! Never reads, stores, logs, or returns a window title, keystroke, or
//! pointer coordinate — the same privacy invariant as
//! `windows_collector`, ported to different OS mechanisms, not a
//! different contract.
//!
//! Every module here is gated behind `#[cfg(target_os = "linux")]` at
//! the `mod` declaration (not with a `not(linux)` stub half in each
//! file, unlike `windows_collector`'s own Windows/not(windows) split) —
//! a deliberate difference, not an inconsistency: `agent-bin`
//! unconditionally depends on `windows_collector` today, so that crate
//! needs a real fallback API on every platform; nothing in this
//! workspace depends on `linux-collector` yet (wiring one unified
//! Linux `agent-bin` variant is a future task, mirroring how
//! `agent-bin` itself only came together in AG-WIN-002, after every
//! Windows building block already existed), so an empty, harmless
//! non-Linux build is sufficient for `cargo build --workspace` to keep
//! succeeding on this project's Windows development host.

#[cfg(target_os = "linux")]
mod collector;
#[cfg(target_os = "linux")]
mod environment;
#[cfg(target_os = "linux")]
mod evdev_counter;
#[cfg(target_os = "linux")]
mod hyprland;
#[cfg(target_os = "linux")]
mod input_counters;
#[cfg(target_os = "linux")]
mod input_events;
#[cfg(target_os = "linux")]
mod native_loop;
#[cfg(target_os = "linux")]
mod process_name;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub use collector::{CollectorError, LinuxSignalCollector};
#[cfg(target_os = "linux")]
pub use environment::{ActiveWindowBackend, UnsupportedReason};
#[cfg(target_os = "linux")]
pub use native_loop::{NativeLoop, NativeLoopError};
