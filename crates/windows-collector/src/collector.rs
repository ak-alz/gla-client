//! `WindowsSignalCollector` — ties `hooks`/`idle`/`foreground`/
//! `browser_title` together into the same `start`/`stop`/`poll` contract
//! as `core/interfaces.py::SignalCollector`, and `poll()` mirrors
//! `WindowsSignalCollector.poll()` in the Python source field-for-field
//! and call-for-call, including the exact order (drain input counters
//! first, then idle, then foreground hwnd, then process name, then —
//! only if both hwnd and process name resolved — the browser-title
//! override).

use crate::browser_title::classify_browser_title;
use crate::foreground::{foreground_hwnd, process_name_for_hwnd};
use crate::hooks::{InputHooks, InputHooksError};
use crate::idle::get_idle_seconds;
use normalization::TitleRules;
use std::collections::HashSet;
use thiserror::Error;

/// What one `poll()` returns — mirrors `core/interfaces.py::RawSignalSnapshot`
/// field-for-field. Never carries a window title or any input value, only
/// counts/flags/the process name — the same architectural privacy boundary
/// as the Python source, ported unchanged rather than reinvented.
#[derive(Debug, Clone)]
pub struct RawSignalSnapshot {
    pub active_process_name: Option<String>,
    pub keyboard_events: i64,
    pub mouse_move_events: i64,
    pub mouse_click_events: i64,
    pub is_idle: bool,
    pub idle_seconds: f64,
    pub category_override: Option<String>,
}

/// Mirrors `core/interfaces.py::SignalCollector` — the platform-agnostic
/// contract every native collector implements. Colocated here (rather
/// than in its own tiny crate) because Windows is the only platform this
/// contract has an implementation for today; if/when AG-LNX-001 or
/// AG-MAC-001 need the same trait, hoisting it into a shared crate at
/// that point is a rename plus a `pub use`, not a redesign — not done
/// preemptively for one implementation (see this project's
/// rule-of-three discipline elsewhere, e.g. `normalization::ALGORITHM_VERSION`'s
/// doc comment).
pub trait SignalCollector {
    fn start(&mut self) -> Result<(), CollectorError>;
    fn stop(&mut self);
    fn poll(&mut self) -> RawSignalSnapshot;
}

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error(transparent)]
    InputHooks(#[from] InputHooksError),
}

pub struct WindowsSignalCollector {
    idle_threshold_seconds: f64,
    browser_process_names: HashSet<String>,
    browser_title_rules: TitleRules,
    hooks: Option<InputHooks>,
}

impl WindowsSignalCollector {
    pub fn new(
        idle_threshold_seconds: f64,
        browser_process_names: HashSet<String>,
        browser_title_rules: TitleRules,
    ) -> Self {
        WindowsSignalCollector {
            idle_threshold_seconds,
            browser_process_names: browser_process_names
                .into_iter()
                .map(|name| name.to_lowercase())
                .collect(),
            browser_title_rules,
            hooks: None,
        }
    }
}

impl SignalCollector for WindowsSignalCollector {
    fn start(&mut self) -> Result<(), CollectorError> {
        self.hooks = Some(InputHooks::start()?);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut hooks) = self.hooks.take() {
            hooks.stop();
        }
    }

    fn poll(&mut self) -> RawSignalSnapshot {
        let (keyboard_events, mouse_move_events, mouse_click_events) =
            crate::hooks::take_and_reset_counts();

        let idle_seconds = get_idle_seconds();
        let is_idle = idle_seconds >= self.idle_threshold_seconds;

        let hwnd = foreground_hwnd();
        let active_process_name = hwnd.and_then(process_name_for_hwnd);
        let category_override = match (hwnd, &active_process_name) {
            (Some(hwnd), Some(process_name)) => classify_browser_title(
                hwnd,
                process_name,
                &self.browser_process_names,
                &self.browser_title_rules,
            ),
            _ => None,
        };

        RawSignalSnapshot {
            active_process_name,
            keyboard_events,
            mouse_move_events,
            mouse_click_events,
            is_idle,
            idle_seconds,
            category_override,
        }
    }
}
