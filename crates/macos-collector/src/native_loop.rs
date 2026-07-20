//! UNVERIFIED — see crate-level doc comment. Translates OS notifications
//! into the SAME `lifecycle::PowerEvent` that `windows-collector`'s and
//! `linux-collector`'s `native_loop.rs` already produce — reusing the
//! existing, already-reviewed `lifecycle::LifecycleState` state machine
//! rather than inventing a third, macOS-specific one (the same reuse
//! argument AG-LNX-002 already made for its own `native_loop.rs`).

use lifecycle::PowerEvent;
use std::sync::mpsc::Receiver;

/// Sleep/wake: `NSWorkspace.shared.notificationCenter` observing
/// `NSWorkspace.willSleepNotification` (→ `PowerEvent::Suspend`) /
/// `.didWakeNotification` (→ `PowerEvent::Resume`).
///
/// Session lock/unlock: **the weakest-confidence row in this whole
/// crate** (see `AGENT_MACOS_CAPABILITY_MATRIX.md`'s capability table) —
/// the documented community technique is a `CFNotificationCenter`
/// "distributed notification" observing the undocumented
/// `com.apple.screenIsLocked`/`com.apple.screenIsUnlocked` names, NOT a
/// first-class Apple-documented API the way Windows'
/// `WTSRegisterSessionNotification` or Linux's `org.freedesktop.login1`
/// D-Bus signals are. Flagged here so whoever implements this for real
/// checks this specific mechanism first, before assuming the rest of
/// this crate's lower-risk rows are equally uncertain.
///
/// UNVERIFIED: never compiled against `objc2-foundation`'s notification-
/// center bindings, nor the raw `CFNotificationCenter` C API this would
/// also need for the lock/unlock half.
pub struct NativeLoop {
    _private: (),
}

#[derive(Debug)]
pub struct NativeLoopError;

impl std::fmt::Display for NativeLoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "macos-collector native_loop: not yet implemented (draft, never compiled)"
        )
    }
}

impl std::error::Error for NativeLoopError {}

impl NativeLoop {
    pub fn start() -> Result<(Self, Receiver<PowerEvent>), NativeLoopError> {
        todo!(
            "register NSWorkspace.willSleepNotification/didWakeNotification observers, \
             and a CFNotificationCenter distributed-notification observer for \
             com.apple.screenIsLocked/screenIsUnlocked, forwarding each into a \
             std::sync::mpsc::Sender<PowerEvent> the returned Receiver reads from — \
             mirrors windows-collector's/linux-collector's native_loop.rs shape, \
             never compiled here"
        )
    }

    pub fn stop(&mut self) {
        todo!("remove the notification observers registered by start()")
    }
}
