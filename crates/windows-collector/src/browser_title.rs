//! The one narrow exception to "never read a window title," ported from
//! `_classify_browser_title` exactly: only reads the title when the caller
//! has configured `browser_title_rules`, and only for a process already
//! known to be a browser. The title string is read into a local buffer,
//! handed to `normalization::classify_title` for a category (or `None`),
//! and dropped at the end of this function — it is never returned, logged,
//! or stored anywhere by any caller.

use normalization::TitleRules;
use std::collections::HashSet;

/// Pure gating logic mirroring `_classify_browser_title`'s two early
/// returns, split out so it's testable without any real window/title.
pub fn should_classify(
    process_name: &str,
    browser_process_names: &HashSet<String>,
    rules: &TitleRules,
) -> bool {
    if rules.is_empty() {
        return false;
    }
    browser_process_names.contains(&process_name.to_lowercase())
}

#[cfg(windows)]
fn read_window_title(hwnd: usize) -> String {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};

    let hwnd = hwnd as HWND;
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if copied <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..copied as usize])
}

/// Mirrors `_classify_browser_title(hwnd, process_name)` end to end: reads
/// the title (a local, function-scoped `String` — see `read_window_title`),
/// classifies it, and returns only the resulting category name.
#[cfg(windows)]
pub fn classify_browser_title(
    hwnd: usize,
    process_name: &str,
    browser_process_names: &HashSet<String>,
    rules: &TitleRules,
) -> Option<String> {
    if !should_classify(process_name, browser_process_names, rules) {
        return None;
    }
    let title = read_window_title(hwnd); // local variable, dropped at end of scope
    normalization::classify_title(Some(&title), rules)
}

/// Non-Windows stub — see `idle::get_idle_seconds`'s doc comment for why
/// this crate still needs to compile (but never run) off Windows.
#[cfg(not(windows))]
pub fn classify_browser_title(
    _hwnd: usize,
    _process_name: &str,
    _browser_process_names: &HashSet<String>,
    _rules: &TitleRules,
) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> TitleRules {
        vec![("media".to_string(), vec!["youtube".to_string()])]
    }

    fn browsers() -> HashSet<String> {
        ["chrome.exe".to_string()].into_iter().collect()
    }

    #[test]
    fn no_rules_means_never_classify() {
        assert!(!should_classify("chrome.exe", &browsers(), &Vec::new()));
    }

    #[test]
    fn non_browser_process_is_never_classified() {
        assert!(!should_classify("notepad.exe", &browsers(), &rules()));
    }

    #[test]
    fn process_name_match_is_case_insensitive() {
        assert!(should_classify("CHROME.EXE", &browsers(), &rules()));
    }

    #[test]
    fn known_browser_with_rules_is_classified() {
        assert!(should_classify("chrome.exe", &browsers(), &rules()));
    }
}
