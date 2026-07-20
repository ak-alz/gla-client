//! UNVERIFIED — see crate-level doc comment.

/// Seconds since the last system-wide input event, without needing to
/// observe individual events (unlike `input_counter.rs`) — no permission
/// required per Apple's documentation. Mirrors the "never `f64::MAX`,
/// always `f64::INFINITY` for 'never observed'" convention already
/// applied on Linux (`linux_collector::input_counters`) if this ever
/// needs a sentinel value for a query failure.
///
/// Documented approach: `CGEventSourceSecondsSinceLastEventType(
/// kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType)` — a
/// single Core Graphics call.
///
/// UNVERIFIED: never compiled against `objc2-core-graphics`'s actual
/// binding for this function (name/signature/constant values as
/// exposed by that crate specifically, vs. the raw C API this is
/// based on).
pub fn idle_seconds() -> f64 {
    todo!(
        "CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType) \
         via objc2-core-graphics — exact binding name never checked against a compiler"
    )
}
