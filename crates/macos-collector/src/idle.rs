//! UNVERIFIED — see crate-level doc comment.

use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};

/// Seconds since the last system-wide input event, without needing to
/// observe individual events (unlike `input_counter.rs`) — no permission
/// required per Apple's documentation.
///
/// `CGEventSourceStateID::HIDSystemState` (not `CombinedSessionState`) —
/// reflects real hardware input regardless of which process/session
/// generated it, the standard choice for system-wide idle detection
/// (matches the convention used by every real macOS idle-time utility
/// this was researched against, see AGENT_MACOS_CAPABILITY_MATRIX.md).
///
/// `CGEventType(u32::MAX)` — the raw value of Apple's `kCGAnyInputEventType`
/// (`(CGEventType)~0`), which `objc2-core-graphics` does not expose as a
/// named constant (it's a C macro, not an exported symbol) — constructed
/// directly since `CGEventType` is a public single-field tuple struct.
///
/// UNVERIFIED: written against the real, cached `objc2-core-graphics`
/// 0.3.2 source (`CGEventSource::seconds_since_last_event_type`,
/// confirmed to exist with this exact signature) — but never compiled or
/// linked on a real Mac.
pub fn idle_seconds() -> f64 {
    CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::HIDSystemState,
        CGEventType(u32::MAX),
    )
}
