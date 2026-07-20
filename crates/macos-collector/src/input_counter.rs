//! UNVERIFIED — see crate-level doc comment.

use crate::permissions::MissingPermission;

/// Keyboard/mouse event counts since the last `take_and_reset()` — the
/// input-count capability, gated behind Accessibility permission (see
/// `permissions::accessibility_trusted`). Per this task's scoping
/// ("input aggregate only if permitted and necessary"), `start()` must
/// be an explicit opt-in the caller decides to attempt, not something
/// tried unconditionally at collector startup alongside
/// `active_app`/`idle` (which need no permission at all) — conflating
/// the two would make a single "no thanks" from the user look like it
/// blocks capabilities that don't actually need Accessibility.
pub struct InputCounter {
    keyboard_events: i64,
    mouse_move_events: i64,
    mouse_click_events: i64,
}

impl InputCounter {
    pub fn new() -> Self {
        Self {
            keyboard_events: 0,
            mouse_move_events: 0,
            mouse_click_events: 0,
        }
    }

    /// Attempts to install a system-wide passive event tap
    /// (`CGEventTapCreate`) purely to COUNT events, never to read/store
    /// their content (no keystrokes, no coordinates persisted — same
    /// privacy contract as `windows-collector`'s `WH_KEYBOARD_LL`/
    /// `WH_MOUSE_LL` hooks and `linux-collector`'s `evdev` reader).
    /// Returns `Err(MissingPermission::AccessibilityNotGranted)` if
    /// `permissions::accessibility_trusted()` is false — must be checked
    /// BEFORE attempting the tap, not discovered via a failed call,
    /// since this task's "graceful degraded mode" acceptance criterion
    /// needs the caller to be able to distinguish "not granted, keep
    /// working without this" from an actual runtime error.
    ///
    /// UNVERIFIED: never compiled against `objc2-core-graphics`'s
    /// `CGEventTapCreate` binding, nor tested for whether a passive tap
    /// scoped only to counting (not modifying/consuming events) is
    /// achievable with the options this crate's dependencies expose.
    pub fn start(&mut self) -> Result<(), MissingPermission> {
        if !crate::permissions::accessibility_trusted() {
            return Err(MissingPermission::AccessibilityNotGranted);
        }
        todo!(
            "CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionListenOnly, \
             mask covering keyDown/mouseMoved/leftMouseDown/rightMouseDown, callback incrementing \
             the three counters below, never persisting event content) via objc2-core-graphics — \
             never checked against a compiler"
        )
    }

    pub fn stop(&mut self) {
        todo!("CFRunLoopSourceInvalidate / CGEventTapEnable(tap, false) — tear down the tap installed by start()")
    }

    pub fn take_and_reset(&mut self) -> (i64, i64, i64) {
        let result = (
            self.keyboard_events,
            self.mouse_move_events,
            self.mouse_click_events,
        );
        self.keyboard_events = 0;
        self.mouse_move_events = 0;
        self.mouse_click_events = 0;
        result
    }
}

impl Default for InputCounter {
    fn default() -> Self {
        Self::new()
    }
}
