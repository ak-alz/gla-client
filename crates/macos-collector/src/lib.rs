//! **AG-MAC-001 — written with no macOS hardware/VM/CI access, ever.**
//! `permissions`/`idle`/`active_app`/`input_counter` now contain real,
//! best-effort implementations (real `objc2-app-kit`/`objc2-core-graphics`/
//! `objc2-core-foundation` 0.3.2 API calls, checked against those crates'
//! actual cached source — not guessed at symbol names — including a
//! background-thread + `CFRunLoop` event-tap pattern mirroring
//! `windows-collector::hooks`'s already-proven `WH_KEYBOARD_LL`/
//! `WH_MOUSE_LL` message-pump structure). `native_loop` remains an
//! intentional skeleton (sleep/wake and lock/unlock power events) — a
//! real implementation needs `objc2-foundation` + `block2` (Objective-C
//! block support), neither of which this crate depended on before, and
//! the lock/unlock half specifically relies on an undocumented
//! `com.apple.screenIsLocked`/`screenIsUnlocked` distributed notification
//! (see that module's own doc comment) — a materially bigger, riskier
//! addition than the four modules above, deliberately left for a
//! follow-up rather than rushed in the same pass.
//!
//! NONE of this has been compiled, linked, or run on a real Mac — there
//! is no macOS hardware/VM available in any environment this crate has
//! ever been written in. Every API call was checked against the real,
//! cached source of the exact crate versions this crate depends on
//! (confirmed to exist with the signatures used), which is a real step
//! up from pure guessing, but is NOT the same thing as compiling. See
//! `docs/02_ARCHITECTURE/AGENT_MACOS_CAPABILITY_MATRIX.md` for the
//! original capability research this is based on.
//!
//! Do not treat anything here as verified. The very first thing whoever
//! next has real Mac hardware should do is `cargo build --target
//! aarch64-apple-darwin -p macos-collector` and fix whatever doesn't
//! compile — expect something not to, on the first try.
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
