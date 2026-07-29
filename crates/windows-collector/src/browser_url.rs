//! Reads the active browser's address-bar text via Windows UI Automation
//! (UIA) — the same accessibility API screen readers use. No browser
//! extension, no cooperation from the browser required; this reads straight
//! from the OS accessibility tree. Exactly the same one-way discipline as
//! `browser_title.rs`: the raw text is handed straight to
//! `normalization::classify_url` (which extracts only the HOST and discards
//! everything else — scheme, path, query string — immediately) and is never
//! returned, stored, or logged by any caller here. A URL can carry far more
//! than a title ever did (paths, query strings, occasionally credentials in
//! edge cases), which is exactly why the extraction step lives in
//! `normalization` and runs on every read, with nothing bypassing it.
//!
//! Empirically verified against real, running Chrome/Edge/Firefox windows on
//! real hardware (not just unit-tested in isolation) — findings this module
//! is built from:
//!   - Chromium family (Chrome/Edge, and by the same shared Views toolkit,
//!     almost certainly Brave/Opera/Vivaldi/Yandex too): the address bar is
//!     the ONE descendant whose UIA ClassName is exactly `"OmniboxViewViews"`
//!     — true even inside a page with 50+ of its own Edit/ComboBox-typed
//!     form fields (confirmed against this very product's own Settings page).
//!   - Firefox: the address bar's UIA AutomationId is `"urlbar-input"` — a
//!     different discriminator; Firefox's UI toolkit isn't Chromium's.
//!   - Firefox's accessibility engine is LAZILY initialized: the very first
//!     UIA query against a freshly-launched Firefox process can legitimately
//!     return nothing (its a11y tree isn't built yet), even though a later
//!     query against the SAME process succeeds. This module does not
//!     retry/block on that — a poll tick that misses is no different from
//!     any other tick where a signal briefly isn't available (matches
//!     `foreground.rs`'s "degrade gracefully, never required" philosophy);
//!     a later poll, once Firefox's engine has warmed up, just works.

use std::collections::HashSet;
use std::ffi::c_void;
use windows::core::{Interface, VARIANT};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_AutomationIdPropertyId, UIA_ClassNamePropertyId, UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId,
    UIA_EditControlTypeId, UIA_ValuePatternId,
};

const CHROMIUM_OMNIBOX_CLASS_NAME: &str = "OmniboxViewViews";
const FIREFOX_URLBAR_AUTOMATION_ID: &str = "urlbar-input";

struct AutomationHandle {
    automation: IUIAutomation,
    address_bar_condition: IUIAutomationCondition,
}

/// Owns the one `IUIAutomation` COM instance (plus its reusable search
/// condition) for the collector's entire lifetime. Created lazily, ON THE
/// POLLING THREAD ITSELF — COM apartment affinity is per-thread, so
/// initialization must happen on whichever thread actually calls `read`,
/// never at construction time from a different thread (in this codebase
/// that's naturally satisfied: `WindowsSignalCollector` is built fresh
/// inside `run_collector_loop`, which already runs entirely on its own
/// spawned thread — see `agent-bin/src/main.rs`). Reused across every
/// subsequent poll: recreating a COM object every tick would be pure
/// per-poll overhead for something that never changes once created.
///
/// Deliberately NOT `Send`/`Sync` (neither is `IUIAutomation` itself, since
/// COM interface pointers aren't safe to move across apartments) — this
/// must never be sent to another thread, and nothing in this codebase
/// tries to.
pub struct AddressBarReader {
    automation: Option<AutomationHandle>,
    /// Set once an initialization attempt has run, success or failure — so
    /// a failed attempt (COM unavailable for some reason) isn't retried on
    /// every single poll forever, matching how every other signal in this
    /// crate fails silently-and-permanently-for-this-run rather than
    /// retrying in a hot loop.
    attempted_init: bool,
}

impl Default for AddressBarReader {
    fn default() -> Self {
        AddressBarReader {
            automation: None,
            attempted_init: false,
        }
    }
}

impl AddressBarReader {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_initialized(&mut self) {
        if self.attempted_init {
            return;
        }
        self.attempted_init = true;
        self.automation = Self::try_initialize().ok();
    }

    fn try_initialize() -> windows::core::Result<AutomationHandle> {
        unsafe {
            // Return value ignored deliberately: `CoInitializeEx` reports
            // S_FALSE (surfaced as an "error" by the `windows` crate's
            // `.ok()` convention) when this thread's apartment was already
            // initialized by other code — a normal, harmless case here,
            // not a real failure worth propagating.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

            // Only Edit/ComboBox-typed controls are candidates at all —
            // narrows the tree walk before the (more expensive) class
            // name / automation ID checks below.
            let edit_cond =
                automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_EditControlTypeId.0))?;
            let combo_cond = automation
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ComboBoxControlTypeId.0))?;
            let control_type_cond = automation.CreateOrCondition(&edit_cond, &combo_cond)?;

            // The actual address-bar discriminator, per real browser engine
            // — see the module doc comment for how these were found.
            let chromium_cond = automation
                .CreatePropertyCondition(UIA_ClassNamePropertyId, &VARIANT::from(CHROMIUM_OMNIBOX_CLASS_NAME))?;
            let firefox_cond = automation
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(FIREFOX_URLBAR_AUTOMATION_ID))?;
            let engine_cond = automation.CreateOrCondition(&chromium_cond, &firefox_cond)?;

            let address_bar_condition = automation.CreateAndCondition(&control_type_cond, &engine_cond)?;

            Ok(AutomationHandle {
                automation,
                address_bar_condition,
            })
        }
    }

    /// Reads the given browser window's address-bar text, or `None` on ANY
    /// failure — COM unavailable, window closed mid-read, address bar not
    /// found this tick (including Firefox's a11y engine not warmed up yet),
    /// etc. Every failure mode collapses to the same "no signal this tick,"
    /// exactly like every other collector signal in this crate. Caller
    /// (`collector.rs`) is responsible for only calling this when `hwnd` is
    /// already known to be a browser window — this function does not
    /// re-check that.
    pub fn read(&mut self, hwnd: usize) -> Option<String> {
        self.ensure_initialized();
        let handle = self.automation.as_ref()?;
        let hwnd = HWND(hwnd as *mut c_void);
        unsafe {
            let root = handle.automation.ElementFromHandle(hwnd).ok()?;
            let element = root
                .FindFirst(TreeScope_Descendants, &handle.address_bar_condition)
                .ok()?;
            let pattern = element.GetCurrentPattern(UIA_ValuePatternId).ok()?;
            let value_pattern: IUIAutomationValuePattern = pattern.cast().ok()?;
            let value = value_pattern.CurrentValue().ok()?;
            let text = value.to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
    }
}

/// Same gating as `browser_title::should_classify` — skip the (real UIA
/// tree walk, not free) read entirely when there are no URL rules
/// configured at all, or the foreground process isn't a known browser.
pub fn should_classify_via_url(
    process_name: &str,
    browser_process_names: &HashSet<String>,
    rules: &normalization::UrlRules,
) -> bool {
    if rules.is_empty() {
        return false;
    }
    browser_process_names.contains(&process_name.to_lowercase())
}

/// Mirrors `browser_title::classify_browser_title` end to end: reads the
/// address bar (a local, function-scoped `String` — see `read`, above),
/// classifies it, and returns the resulting category name plus which
/// keyword matched (the keyword is the user's own rule text, not the
/// address — see `normalization::classify_url_with_match`'s doc comment).
/// Only called when `should_read` (the same gating as
/// `browser_title::should_classify`, reused directly — see `collector.rs`)
/// is true.
pub fn classify_browser_url(
    reader: &mut AddressBarReader,
    hwnd: usize,
    rules: &normalization::UrlRules,
) -> Option<(String, String)> {
    let text = reader.read(hwnd)?; // local variable, dropped at end of scope
    normalization::classify_url_with_match(Some(&text), rules)
}
