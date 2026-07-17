//! Windows platform signal collector (AG-WIN-001): a Rust port of
//! `agent/platforms/windows/collector.py`'s `WindowsSignalCollector`, plus
//! real OS registration for session lock/unlock and power suspend/resume
//! events (`native_loop`), which `lifecycle::power_events::LifecycleState`
//! (AG-008) documented as a deliberate follow-up rather than in scope for
//! that crate. See each module's doc comment for exactly what's pure and
//! unit-tested everywhere vs. Windows-only and verified via real
//! (non-mocked) OS calls — either in `#[cfg(all(test, windows))]` tests or
//! `examples/collector_demo.rs`, per module.
//!
//! Never reads, stores, logs, or returns a window title, keystroke, mouse
//! position, or screen content — the one narrow, deliberate exception
//! (local browser-title classification, `browser_title.rs`) reads a title
//! only to hand it to `normalization::classify_title` and immediately
//! discards it, exactly mirroring the Python source's own architecture
//! comment on `platforms/windows/collector.py`.

mod browser_title;
mod collector;
mod foreground;
mod hooks;
mod idle;
mod input_counters;
mod native_loop;

pub use collector::{CollectorError, RawSignalSnapshot, SignalCollector, WindowsSignalCollector};
pub use hooks::InputHooksError;
pub use native_loop::{LifecycleNotification, NativeLoop, NativeLoopError};
