//! Pure counting state — same design as
//! `windows_collector::input_counters::InputCounters` (atomics, not a
//! mutex, since the OS-facing reader thread in `evdev_counter.rs` only
//! ever writes and `poll()` only ever reads-and-resets) — duplicated
//! rather than shared cross-crate because each platform crate owns its
//! own small OS-adjacent helpers independently (only the top-level
//! `collector_core::SignalCollector` contract is actually shared).
//!
//! Also tracks the last-observed-event instant, used by the Hyprland/
//! Wayland idle-time fallback (`collector.rs`): X11 has the real,
//! system-wide XScreenSaver counter (authoritative regardless of
//! whether this collector was running), but no equivalent exists for
//! Wayland within this task's scope (`ext-idle-notify-v1` needs a full
//! Wayland protocol client — out of scope, see
//! AGENT_LINUX_CAPABILITY_MATRIX.md) — deriving idle time from "seconds
//! since evdev last observed ANY input" is a real, working mechanism,
//! just one that is only accurate from this collector's own start
//! (never having observed an event yet is treated as maximally idle,
//! the safer of the two possible wrong defaults — see `idle_seconds`).

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Default)]
pub struct InputCounters {
    keyboard: AtomicI64,
    mouse_move: AtomicI64,
    mouse_click: AtomicI64,
    last_event_at: Mutex<Option<Instant>>,
}

impl InputCounters {
    pub fn new() -> Self {
        InputCounters {
            keyboard: AtomicI64::new(0),
            mouse_move: AtomicI64::new(0),
            mouse_click: AtomicI64::new(0),
            last_event_at: Mutex::new(None),
        }
    }

    pub fn record(&self, kind: crate::input_events::EventKind) {
        use crate::input_events::EventKind;
        match kind {
            EventKind::Keyboard => self.keyboard.fetch_add(1, Ordering::Relaxed),
            EventKind::MouseMove => self.mouse_move.fetch_add(1, Ordering::Relaxed),
            EventKind::MouseClick => self.mouse_click.fetch_add(1, Ordering::Relaxed),
        };
        *self.last_event_at.lock().unwrap() = Some(Instant::now());
    }

    pub fn take_and_reset(&self) -> (i64, i64, i64) {
        (
            self.keyboard.swap(0, Ordering::Relaxed),
            self.mouse_move.swap(0, Ordering::Relaxed),
            self.mouse_click.swap(0, Ordering::Relaxed),
        )
    }

    /// Seconds since the last observed event, or `f64::INFINITY` if none
    /// has ever been observed — deliberately the "maximally idle" default,
    /// not "just active": claiming freshly-started activity with zero
    /// evidence would silently overcount active time, while overstating
    /// idleness at worst undercounts a few real seconds right after
    /// startup, matching this project's established fail-safe direction
    /// (see `windows_collector::hooks`'s "always forward the event"
    /// safety property for the same kind of conservative-default
    /// reasoning applied to a different risk).
    pub fn idle_seconds(&self) -> f64 {
        match *self.last_event_at.lock().unwrap() {
            Some(instant) => instant.elapsed().as_secs_f64(),
            None => f64::INFINITY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_events::EventKind;

    #[test]
    fn records_and_resets_independently_per_kind() {
        let counters = InputCounters::new();
        counters.record(EventKind::Keyboard);
        counters.record(EventKind::Keyboard);
        counters.record(EventKind::MouseMove);
        counters.record(EventKind::MouseClick);
        counters.record(EventKind::MouseClick);
        counters.record(EventKind::MouseClick);
        assert_eq!(counters.take_and_reset(), (2, 1, 3));
        assert_eq!(counters.take_and_reset(), (0, 0, 0));
    }
}
