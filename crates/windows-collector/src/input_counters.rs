//! Pure event-counting state, deliberately separated from the OS hook
//! plumbing in `hooks.rs` so the counting/reset logic itself is testable
//! everywhere (not just on a real Windows box with real hooks installed).
//! Mirrors `WindowsSignalCollector`'s `_keyboard_events`/`_mouse_move_events`/
//! `_mouse_click_events` fields plus its `_lock`-guarded increment/drain —
//! atomics replace the Python mutex, since the hook callbacks that will
//! drive this (see `hooks.rs`) are plain `extern "system" fn`s with no
//! captured state, so a `Mutex<T>` behind a shared reference would need the
//! same `'static` placement anyway. Only ever counts events, never stores a
//! key, button, or coordinate — matching the Python source's explicit
//! privacy invariant exactly.

use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Debug, Default)]
pub struct InputCounters {
    keyboard: AtomicI64,
    mouse_move: AtomicI64,
    mouse_click: AtomicI64,
}

impl InputCounters {
    pub const fn new() -> Self {
        InputCounters {
            keyboard: AtomicI64::new(0),
            mouse_move: AtomicI64::new(0),
            mouse_click: AtomicI64::new(0),
        }
    }

    // These three are only called from `hooks.rs`'s `#[cfg(windows)]`
    // hook procedures in production — on other platforms they're
    // exercised solely by this file's own tests, hence the `dead_code`
    // allowance.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn record_keyboard(&self) {
        self.keyboard.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn record_mouse_move(&self) {
        self.mouse_move.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn record_mouse_click(&self) {
        self.mouse_click.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the three counts since the last call and resets them to
    /// zero, matching `poll()`'s exact "read then zero, under one lock"
    /// semantics from the Python source (here: one independent swap per
    /// counter — no cross-counter atomicity is needed since nothing ever
    /// reads a relationship between them, only their individual totals).
    pub fn take_and_reset(&self) -> (i64, i64, i64) {
        (
            self.keyboard.swap(0, Ordering::Relaxed),
            self.mouse_move.swap(0, Ordering::Relaxed),
            self.mouse_click.swap(0, Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero() {
        let counters = InputCounters::new();
        assert_eq!(counters.take_and_reset(), (0, 0, 0));
    }

    #[test]
    fn records_are_counted_independently_per_kind() {
        let counters = InputCounters::new();
        counters.record_keyboard();
        counters.record_keyboard();
        counters.record_mouse_move();
        counters.record_mouse_click();
        counters.record_mouse_click();
        counters.record_mouse_click();
        assert_eq!(counters.take_and_reset(), (2, 1, 3));
    }

    #[test]
    fn take_and_reset_zeroes_the_counters() {
        let counters = InputCounters::new();
        counters.record_keyboard();
        counters.take_and_reset();
        assert_eq!(
            counters.take_and_reset(),
            (0, 0, 0),
            "a second take_and_reset with no new events in between must read zero"
        );
    }

    #[test]
    fn events_after_a_take_and_reset_start_a_fresh_window() {
        let counters = InputCounters::new();
        counters.record_mouse_move();
        counters.take_and_reset();
        counters.record_mouse_move();
        counters.record_mouse_move();
        assert_eq!(counters.take_and_reset(), (0, 2, 0));
    }
}
