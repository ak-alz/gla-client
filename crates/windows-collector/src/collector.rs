//! `WindowsSignalCollector` — ties `hooks`/`idle`/`foreground`/
//! `browser_title` together into the same `start`/`stop`/`poll` contract
//! as `core/interfaces.py::SignalCollector` (now `collector_core::
//! SignalCollector`, shared with `linux-collector` since AG-LNX-002 —
//! see that crate's doc comment), and `poll()` mirrors
//! `WindowsSignalCollector.poll()` in the Python source field-for-field
//! and call-for-call, including the exact order (drain input counters
//! first, then idle, then foreground hwnd, then process name, then —
//! only if both hwnd and process name resolved — the browser-title
//! override).

use crate::browser_title::classify_browser_title;
use crate::foreground::{foreground_hwnd, process_name_for_hwnd};
use crate::hooks::{InputHooks, InputHooksError};
use crate::idle::get_idle_seconds;
use collector_core::{RawSignalSnapshot, SignalCollector};
use normalization::TitleRules;
use std::collections::HashSet;
use thiserror::Error;

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
    type Error = CollectorError;

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
