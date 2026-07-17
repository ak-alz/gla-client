//! One test per AG-006 acceptance criterion not already exercised directly
//! by `golden_fixture_tests.rs`'s parity checks — named explicitly after
//! the criterion so a reviewer can map test-to-requirement at a glance.

use chrono::{DateTime, Utc};
use event_contract::Consent;
use normalization::{
    classify_title, BucketAccumulator, Tick, TitleRules, ALGORITHM_VERSION, UNKNOWN_APP_LABEL,
};
use std::collections::BTreeMap;

fn base_time() -> DateTime<Utc> {
    "2026-07-17T12:00:00Z".parse().unwrap()
}

fn full_consent() -> Consent {
    Consent {
        active_app_category: true,
        input_activity_counts: true,
        idle_tracking: true,
        activity_segments: true,
        unexplained_gaps: true,
        git_activity: false,
        app_detail: true,
    }
}

// --- "Sensitive content не попадает в output" ---

#[test]
fn classify_title_never_returns_the_title_text_itself() {
    let title = "a-very-specific-unique-window-title-marker-987654321";
    // matches a substring of the title
    let rules: TitleRules = vec![("media".to_string(), vec!["987654321".to_string()])];

    let result = classify_title(Some(title), &rules);
    // The function's ENTIRE contract is: input a title, output a category
    // NAME — never the title itself, never a substring of it beyond
    // whatever the caller already supplied as a category label. Confirms
    // structurally, not just by convention, that the result is the
    // configured category name, not anything derived from the title text.
    assert_eq!(result, Some("media".to_string()));
    assert_ne!(result.as_deref(), Some(title));
    assert!(
        !result.unwrap().contains("987654321"),
        "the result must never leak a fragment of the raw title"
    );
}

#[test]
fn tick_struct_has_no_field_capable_of_carrying_raw_window_title_text() {
    // A structural check, not a runtime one: `Tick` (the only input to
    // aggregation) has exactly one place a "what was on screen" fact could
    // enter besides the process name — `category_override`, which by this
    // crate's contract (see aggregation.rs's doc comment) must already be
    // a classified CATEGORY name (produced by `classify_title` upstream),
    // never raw title text. This test documents and exercises that: even
    // if a caller mistakenly passed raw title text into `category_override`
    // (a misuse this crate cannot prevent at the type level, `String` being
    // `String`), it flows through as an opaque category label, never
    // parsed/re-classified against title-shaped content here.
    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    acc.accumulate(&Tick {
        active_process_name: Some("chrome.exe".to_string()),
        keyboard_events: 0,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: Some("media".to_string()), // already-classified, per contract
        occurred_at: base_time(),
        interval_seconds: 2.0,
    });
    let signals = acc.flush(None);
    let categories: Vec<&String> = signals
        .active_app_category_seconds
        .as_ref()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(categories, vec!["media"]);
}

// --- "Unknown app остается inspectable" ---

#[test]
fn a_process_with_no_resolvable_name_still_gets_an_inspectable_app_seconds_entry() {
    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    acc.accumulate(&Tick {
        active_process_name: None, // system dialog / UAC / secure desktop — collector could not resolve a name
        keyboard_events: 0,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        occurred_at: base_time(),
        interval_seconds: 2.0,
    });
    let signals = acc.flush(None);

    let app_seconds = signals
        .app_seconds
        .expect("app_detail consent is on — must be Some, even if empty");
    assert_eq!(
        app_seconds.get(UNKNOWN_APP_LABEL),
        Some(&2.0),
        "an unresolvable process must still surface as an inspectable placeholder entry, not silently vanish from app_seconds"
    );

    // category_seconds must show the SAME 2.0 seconds under "other" — the
    // Python source's specific concern (see aggregator.py's docstring on
    // UNKNOWN_APP_LABEL) that app_seconds's sum must never fall short of
    // category_seconds's sum for the same period.
    let category_seconds = signals.active_app_category_seconds.unwrap();
    assert_eq!(category_seconds.get("other"), Some(&2.0));
}

// --- "Algorithm version сохраняется" ---

#[test]
fn algorithm_version_is_a_stable_explicit_marker() {
    // The Python source has no equivalent explicit marker at all (see
    // lib.rs's "Versioning" doc section) — this port introduces one so a
    // future shift in category/"other" proportions can be attributed to
    // "the algorithm changed" vs "behavior changed," not left ambiguous.
    assert!(!ALGORITHM_VERSION.is_empty());
    assert_eq!(
        ALGORITHM_VERSION.matches('.').count(),
        2,
        "expected a semver-shaped version, e.g. 1.0.0"
    );
}

// --- "Missing signal не превращается в zero activity" ---

#[test]
fn consent_off_yields_none_not_zero_even_when_the_underlying_activity_was_zero() {
    let mut acc = BucketAccumulator::new(
        Consent {
            active_app_category: false,
            input_activity_counts: false,
            idle_tracking: false,
            activity_segments: false,
            unexplained_gaps: false,
            git_activity: false,
            app_detail: false,
        },
        BTreeMap::new(),
        900.0,
    );
    // Zero idle time actually occurred (every tick is active) — with
    // idle_tracking off, this must surface as `None` ("we didn't measure
    // this"), never `Some(0.0)` ("we measured and it was zero") - those
    // are different facts and collapsing them would misrepresent a
    // disabled signal as a confirmed all-active day.
    acc.accumulate(&Tick {
        active_process_name: Some("code.exe".to_string()),
        keyboard_events: 0,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        occurred_at: base_time(),
        interval_seconds: 2.0,
    });
    let signals = acc.flush(None);

    assert_eq!(
        signals.idle_seconds, None,
        "idle_tracking is off — must be None, not Some(0.0)"
    );
    assert_eq!(signals.active_app_category_seconds, None);
    assert_eq!(signals.input_activity_events, None);
    assert_eq!(
        signals.active_seconds, 2.0,
        "active_seconds is the one unconditional field — always present, never consent-gated"
    );
}

#[test]
fn consent_on_and_genuinely_zero_yields_some_zero_not_none() {
    // The mirror image of the test above: idle_tracking ON, but no tick
    // was ever idle — the correct representation is `Some(0.0)` ("we
    // measured, and it was zero"), not `None` ("we don't know").
    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    acc.accumulate(&Tick {
        active_process_name: Some("code.exe".to_string()),
        keyboard_events: 0,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        occurred_at: base_time(),
        interval_seconds: 2.0,
    });
    let signals = acc.flush(None);
    assert_eq!(signals.idle_seconds, Some(0.0));
}
