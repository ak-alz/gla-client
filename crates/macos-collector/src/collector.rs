//! UNVERIFIED — see crate-level doc comment.

use crate::input_counter::InputCounter;
use crate::permissions::MissingPermission;
use collector_core::{RawSignalSnapshot, SignalCollector};

#[derive(Debug)]
pub enum MacosCollectorError {
    /// Mirrors `linux_collector`'s `LinuxSignalCollector::start()`
    /// returning an explicit error rather than silently degrading —
    /// reserved for a genuine startup failure (not the same thing as
    /// `MissingPermission`, which is an expected, common, gracefully-
    /// handled state per this task's acceptance criteria, not an error).
    StartupFailed(String),
}

impl std::fmt::Display for MacosCollectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StartupFailed(msg) => write!(f, "macos-collector startup failed: {msg}"),
        }
    }
}

impl std::error::Error for MacosCollectorError {}

/// Draft `SignalCollector` implementation for macOS. `input_counter` is
/// `None` whenever Accessibility permission isn't granted (checked once
/// at `start()`, matching this task's "no unnecessary permission" +
/// "permission denial has graceful degraded mode" acceptance criteria —
/// `active_process_name`/`is_idle`/`idle_seconds` keep working with zero
/// permission regardless of whether Accessibility was ever granted, only
/// the three input-count fields go to `0` when it isn't).
pub struct MacosSignalCollector {
    input_counter: Option<InputCounter>,
}

impl MacosSignalCollector {
    pub fn new() -> Self {
        Self {
            input_counter: None,
        }
    }

    /// The one place this crate's caller learns WHY input counting isn't
    /// active, if it isn't — matching `linux_collector::environment`'s
    /// `UnsupportedReason` honesty pattern instead of a silent zero with
    /// no explanation reaching the diagnostics surface.
    pub fn input_counting_status(&self) -> Result<(), MissingPermission> {
        if self.input_counter.is_some() {
            Ok(())
        } else {
            Err(MissingPermission::AccessibilityNotGranted)
        }
    }
}

impl Default for MacosSignalCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalCollector for MacosSignalCollector {
    type Error = MacosCollectorError;

    fn start(&mut self) -> Result<(), Self::Error> {
        // Input counting is opt-in and permission-gated — a denial here
        // is NOT a startup failure, it's the expected common case this
        // task's acceptance criteria explicitly asks to degrade
        // gracefully from.
        let mut counter = InputCounter::new();
        if counter.start().is_ok() {
            self.input_counter = Some(counter);
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(counter) = self.input_counter.as_mut() {
            counter.stop();
        }
    }

    fn poll(&mut self) -> RawSignalSnapshot {
        let (keyboard_events, mouse_move_events, mouse_click_events) = self
            .input_counter
            .as_mut()
            .map(|c| c.take_and_reset())
            .unwrap_or((0, 0, 0));

        let idle_seconds = crate::idle::idle_seconds();

        RawSignalSnapshot {
            active_process_name: crate::active_app::frontmost_process_name(),
            keyboard_events,
            mouse_move_events,
            mouse_click_events,
            is_idle: idle_seconds >= 120.0, // matches the 120s threshold windows-collector/linux-collector already use
            idle_seconds,
            category_override: None,
        }
    }
}
