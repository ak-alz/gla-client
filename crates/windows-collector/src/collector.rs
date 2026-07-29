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
use crate::browser_url::{classify_browser_url, AddressBarReader};
use crate::foreground::{foreground_hwnd, process_name_for_hwnd};
use crate::hooks::{InputHooks, InputHooksError};
use crate::idle::get_idle_seconds;
use collector_core::{RawSignalSnapshot, SignalCollector};
use normalization::{TitleRules, UrlRules};
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
    browser_url_rules: UrlRules,
    /// Company Layer — completely independent rule sets, never merged
    /// with the personal ones above (see `Tick::company_category`'s
    /// doc comment in `normalization::aggregation`).
    company_browser_title_rules: TitleRules,
    company_browser_url_rules: UrlRules,
    address_bar_reader: AddressBarReader,
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
            browser_url_rules: Vec::new(),
            company_browser_title_rules: Vec::new(),
            company_browser_url_rules: Vec::new(),
            address_bar_reader: AddressBarReader::new(),
            hooks: None,
        }
    }

    /// Refreshes the title-classification rules on an already-running
    /// collector — added so `agent-bin`'s periodic poll of `GET
    /// /v1/agent/title-rules` (real, user-defined rules like "youtube"
    /// -> a custom "Отдых" category) can take effect without tearing
    /// down and rebuilding the collector, same reasoning as
    /// `normalization::BucketAccumulator::set_category_overrides`.
    pub fn set_browser_title_rules(&mut self, browser_title_rules: TitleRules) {
        self.browser_title_rules = browser_title_rules;
    }

    /// Same idea as `set_browser_title_rules`, for `GET /v1/agent/url-rules`
    /// (real, user-defined domain rules like "youtube" -> a custom "Отдых"
    /// category, matched against the address bar instead of the title).
    pub fn set_browser_url_rules(&mut self, browser_url_rules: UrlRules) {
        self.browser_url_rules = browser_url_rules;
    }

    /// Company Layer — same refresh mechanism as `set_browser_title_rules`,
    /// for `GET /v1/agent/company-title-rules`. Never merged with the
    /// personal rule set above.
    pub fn set_company_browser_title_rules(&mut self, rules: TitleRules) {
        self.company_browser_title_rules = rules;
    }

    /// Company Layer counterpart of `set_browser_url_rules`, for
    /// `GET /v1/agent/company-url-rules`.
    pub fn set_company_browser_url_rules(&mut self, rules: UrlRules) {
        self.company_browser_url_rules = rules;
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
        // URL rules are tried FIRST when they're configured and match —
        // an address-bar host is a more precise signal than a title
        // keyword now that we actually have it (see `browser_url.rs`'s
        // doc comment for how it's read). Title rules remain the
        // fallback: they still fire on their own configured keywords when
        // no URL rule matched (or URL reading failed this tick — e.g. a
        // freshly-launched Firefox process whose accessibility engine
        // hasn't warmed up yet), so nothing regresses for users who only
        // ever set up title rules.
        // `(category, "title:<keyword>" | "url:<keyword>")` — the prefix
        // records WHICH kind of rule fired, not just that one did, so
        // `rule_match_seconds` (see aggregation.rs) can tell a title match
        // apart from a URL match on the same keyword text.
        let matched: Option<(String, String)> = match (hwnd, &active_process_name) {
            (Some(hwnd), Some(process_name)) => {
                let should_read = crate::browser_url::should_classify_via_url(
                    process_name,
                    &self.browser_process_names,
                    &self.browser_url_rules,
                );
                let from_url = if should_read {
                    classify_browser_url(&mut self.address_bar_reader, hwnd, &self.browser_url_rules)
                        .map(|(category, keyword)| (category, format!("url:{keyword}")))
                } else {
                    None
                };
                from_url.or_else(|| {
                    classify_browser_title(
                        hwnd,
                        process_name,
                        &self.browser_process_names,
                        &self.browser_title_rules,
                    )
                    .map(|(category, keyword)| (category, format!("title:{keyword}")))
                })
            }
            _ => None,
        };
        let category_override = matched.as_ref().map(|(category, _)| category.clone());
        let matched_rule_key = matched.map(|(_, rule_key)| rule_key);

        // Company Layer — a SECOND, fully independent classification
        // pass over the same (hwnd, process_name), against
        // company/department rules only. Never falls back to the
        // personal `matched` result above, never feeds into it — see
        // `normalization::Tick::company_category`'s doc comment for why
        // (comparable company-wide aggregates need every employee
        // bucketed by the exact same rule set, regardless of what
        // personal rules any one of them has configured).
        let company_category: Option<String> = match (hwnd, &active_process_name) {
            (Some(hwnd), Some(process_name)) => {
                let should_read_company_url = crate::browser_url::should_classify_via_url(
                    process_name,
                    &self.browser_process_names,
                    &self.company_browser_url_rules,
                );
                let from_company_url = if should_read_company_url {
                    classify_browser_url(&mut self.address_bar_reader, hwnd, &self.company_browser_url_rules)
                        .map(|(category, _keyword)| category)
                } else {
                    None
                };
                from_company_url.or_else(|| {
                    classify_browser_title(
                        hwnd,
                        process_name,
                        &self.browser_process_names,
                        &self.company_browser_title_rules,
                    )
                    .map(|(category, _keyword)| category)
                })
            }
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
            matched_rule_key,
            company_category,
        }
    }
}
