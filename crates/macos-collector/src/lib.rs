//! **AG-MAC-001 draft — never compiled, never run.** Written with no
//! macOS hardware/VM/CI access in this session; every module below is a
//! structural skeleton (real `collector_core::SignalCollector` wiring,
//! real dependency crate names confirmed to exist on crates.io) with
//! `todo!()` bodies where the actual macOS API call would go, plus a doc
//! comment on each describing the specific, researched Apple API this
//! is meant to call. See `docs/02_ARCHITECTURE/AGENT_MACOS_CAPABILITY_MATRIX.md`
//! for the full research this skeleton is based on, including an honest
//! list of what could NOT be verified.
//!
//! This is deliberately more conservative than `linux-collector`'s (or
//! `windows-collector`'s) doc comments: those crates document real,
//! measured behavior. Every claim here is a hypothesis for whoever next
//! touches this crate on real hardware, not a verified fact — do not
//! promote any function here past `todo!()` without first getting it
//! compiling and running on a real Mac.
//!
//! Every `mod` is gated `#[cfg(target_os = "macos")]`, the same pattern
//! `linux-collector` uses (nothing in the workspace unconditionally
//! depends on this crate yet) — on Windows/Linux this crate compiles to
//! an empty, harmless shell; confirmed by `cargo build/test/clippy
//! --workspace` passing cleanly on both in this session.

#[cfg(target_os = "macos")]
mod active_app;
#[cfg(target_os = "macos")]
mod collector;
#[cfg(target_os = "macos")]
mod idle;
#[cfg(target_os = "macos")]
mod input_counter;
#[cfg(target_os = "macos")]
mod native_loop;
#[cfg(target_os = "macos")]
mod permissions;

#[cfg(target_os = "macos")]
pub use collector::{MacosCollectorError, MacosSignalCollector};
#[cfg(target_os = "macos")]
pub use collector_core::{RawSignalSnapshot, SignalCollector};
#[cfg(target_os = "macos")]
pub use permissions::MissingPermission;
