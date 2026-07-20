//! The one place `main.rs` picks a concrete platform implementation —
//! everything else in this binary talks only to `collector_core::
//! SignalCollector` and `lifecycle::PowerEvent`, never to a
//! `windows_collector`/`linux_collector` type by name. Exactly one of
//! the two blocks below compiles for any given target (mutually
//! exclusive `cfg`s), so `new_collector()`/`NativeLoop` resolve to a
//! single concrete type at compile time — no `dyn` dispatch needed, and
//! no possibility of accidentally linking both platforms' collectors
//! into one binary.
//!
//! Added in AG-LNX-003 alongside real Linux packaging — before this,
//! `agent-bin` (AG-WIN-002) only ever built against `windows_collector`
//! directly; this module is the minimal change needed to let the SAME
//! binary crate also produce a real Linux agent, now that
//! `linux_collector` (AG-LNX-002) exists.

#[cfg(windows)]
pub use windows_collector::NativeLoop;

#[cfg(windows)]
pub fn new_collector() -> windows_collector::WindowsSignalCollector {
    windows_collector::WindowsSignalCollector::new(
        120.0,
        std::collections::HashSet::new(),
        Vec::new(),
    )
}

#[cfg(target_os = "linux")]
pub use linux_collector::NativeLoop;

#[cfg(target_os = "linux")]
pub fn new_collector() -> linux_collector::LinuxSignalCollector {
    linux_collector::LinuxSignalCollector::new(120.0)
}

/// AG-MAC-002 wires `macos-collector` in here the same way AG-LNX-003
/// wired `linux_collector` above -- but unlike the other two branches,
/// every real API call inside `macos-collector` is still a `todo!()`
/// stub (AG-MAC-001, written and never compiled without real macOS
/// hardware). Compiling `agent-bin` for macOS is therefore expected to
/// build but panic at runtime the first time the collector/native loop
/// actually run -- this wiring exists so the architecture is complete
/// and the remaining work for whoever has real hardware is filling in
/// `macos-collector`'s bodies, not also rebuilding this integration.
#[cfg(target_os = "macos")]
pub use macos_collector::NativeLoop;

#[cfg(target_os = "macos")]
pub fn new_collector() -> macos_collector::MacosSignalCollector {
    macos_collector::MacosSignalCollector::new()
}
