//! UNVERIFIED — see crate-level doc comment.

use crate::browser_url::{classify_browser_url, should_classify_via_url, AddressBarReader};
use crate::input_counter::InputCounter;
use crate::permissions::MissingPermission;
use collector_core::{RawSignalSnapshot, SignalCollector};
use normalization::UrlRules;
use std::collections::HashSet;

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
    browser_process_names: HashSet<String>,
    browser_url_rules: UrlRules,
    /// Company Layer — independent from `browser_url_rules` above, see
    /// `normalization::Tick::company_category`'s doc comment.
    company_browser_url_rules: UrlRules,
    address_bar_reader: AddressBarReader,
}

impl MacosSignalCollector {
    pub fn new() -> Self {
        Self {
            input_counter: None,
            browser_process_names: [
                "safari", "chrome", "google chrome", "firefox", "brave browser", "microsoft edge",
            ]
            .into_iter()
            .map(|name| name.to_lowercase())
            .collect(),
            browser_url_rules: Vec::new(),
            company_browser_url_rules: Vec::new(),
            address_bar_reader: AddressBarReader::new(),
        }
    }

    /// Same idea as `windows_collector::WindowsSignalCollector::
    /// set_browser_url_rules` — refreshes URL-classification rules on an
    /// already-running collector.
    pub fn set_browser_url_rules(&mut self, browser_url_rules: UrlRules) {
        self.browser_url_rules = browser_url_rules;
    }

    /// Company Layer counterpart — see
    /// `windows_collector::WindowsSignalCollector::set_company_browser_url_rules`.
    pub fn set_company_browser_url_rules(&mut self, rules: UrlRules) {
        self.company_browser_url_rules = rules;
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
        let active_process_name = crate::active_app::frontmost_process_name();

        // Same "URL rules first, no title-based fallback here" shape as
        // Linux (this platform never had title classification either) —
        // see `browser_url.rs`'s doc comment for the real caveat: this
        // whole path is UNVERIFIED, never compiled/linked on real macOS
        // hardware.
        let matched: Option<(String, String)> = match &active_process_name {
            Some(process_name)
                if should_classify_via_url(process_name, &self.browser_process_names, &self.browser_url_rules) =>
            {
                crate::active_app::frontmost_pid()
                    .and_then(|pid| classify_browser_url(&mut self.address_bar_reader, pid, &self.browser_url_rules))
                    .map(|(category, keyword)| (category, format!("url:{keyword}")))
            }
            _ => None,
        };
        let category_override = matched.as_ref().map(|(category, _)| category.clone());
        let matched_rule_key = matched.map(|(_, rule_key)| rule_key);

        // Company Layer — second, independent classification pass, see
        // `windows_collector::collector`'s poll() for the shared
        // reasoning.
        let company_category: Option<String> = match &active_process_name {
            Some(process_name)
                if should_classify_via_url(process_name, &self.browser_process_names, &self.company_browser_url_rules) =>
            {
                crate::active_app::frontmost_pid()
                    .and_then(|pid| classify_browser_url(&mut self.address_bar_reader, pid, &self.company_browser_url_rules))
                    .map(|(category, _keyword)| category)
            }
            _ => None,
        };

        RawSignalSnapshot {
            active_process_name,
            keyboard_events,
            mouse_move_events,
            mouse_click_events,
            is_idle: idle_seconds >= 120.0, // matches the 120s threshold windows-collector/linux-collector already use
            idle_seconds,
            category_override,
            matched_rule_key,
            company_category,
        }
    }
}
