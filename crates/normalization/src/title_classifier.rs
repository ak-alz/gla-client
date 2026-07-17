//! Direct port of `agent/core/title_classifier.py`. This is the ONLY place
//! in the whole agent where window title TEXT is ever examined —
//! `classify_title` takes a `&str` and rules, and returns ONLY a category
//! name (or nothing). The title itself must never be returned, stored, or
//! logged by ANY caller of this function — that architectural constraint
//! (see the Python docstring this ports) is enforced by convention here
//! exactly as it was in Python, not by a new mechanism; this port changes
//! nothing about that guarantee, it only changes the implementation language.

/// Ordered list of (category, patterns) — a `Vec`, not a `HashMap`/`BTreeMap`,
/// because ORDER IS SEMANTICALLY LOAD-BEARING here: Python dicts preserve
/// insertion order, and `classify_title` returns the FIRST matching
/// category in that order when multiple categories' patterns could match
/// the same title. A hash map would silently reorder this and change which
/// category wins — confirmed by golden fixtures where swapping two
/// categories' order in otherwise-identical rules flips the result (see
/// `tests/golden_fixture_tests.rs`).
pub type TitleRules = Vec<(String, Vec<String>)>;

/// Returns the first matching category by substring (case-insensitive), or
/// `None` if there are no rules or nothing matched.
pub fn classify_title(title: Option<&str>, rules: &TitleRules) -> Option<String> {
    let title = title?;
    if title.is_empty() || rules.is_empty() {
        return None;
    }

    let lowered = title.to_lowercase();
    for (category, patterns) in rules {
        for pattern in patterns {
            if !pattern.is_empty() && lowered.contains(&pattern.to_lowercase()) {
                return Some(category.clone());
            }
        }
    }
    None
}
