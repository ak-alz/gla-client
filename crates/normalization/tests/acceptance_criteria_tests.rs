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
        matched_rule_key: None,
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
        matched_rule_key: None,
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

// --- Schema 0.6.0: category_app_seconds / rule_match_seconds ---

#[test]
fn category_app_seconds_reflects_a_rule_reclassification_the_backend_could_not_derive_from_app_seconds_alone() {
    // The exact real bug this field fixes: chrome.exe's time is split
    // between two categories in the SAME bucket (some reclassified by a
    // URL rule to "rest", the rest staying "browser") — app_seconds alone
    // (just "chrome.exe: 4.0") can never tell those apart after the fact,
    // only the agent, which saw the per-tick resolution, can.
    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    acc.accumulate(&Tick {
        active_process_name: Some("chrome.exe".to_string()),
        keyboard_events: 0,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: Some("rest".to_string()),
        matched_rule_key: Some("url:youtube".to_string()),
        occurred_at: base_time(),
        interval_seconds: 2.0,
    });
    acc.accumulate(&Tick {
        active_process_name: Some("chrome.exe".to_string()),
        keyboard_events: 0,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: Some("browser".to_string()),
        matched_rule_key: None,
        occurred_at: base_time() + chrono::Duration::seconds(2),
        interval_seconds: 2.0,
    });
    let signals = acc.flush(None);

    let category_app_seconds = signals
        .category_app_seconds
        .expect("app_detail consent is on — must be Some");
    assert_eq!(
        category_app_seconds.get("rest").and_then(|m| m.get("chrome.exe")),
        Some(&2.0),
        "the reclassified 2s must show up under \"rest\", not lumped into \"browser\""
    );
    assert_eq!(
        category_app_seconds.get("browser").and_then(|m| m.get("chrome.exe")),
        Some(&2.0),
        "the NON-reclassified 2s must stay under \"browser\""
    );

    let rule_match_seconds = signals
        .rule_match_seconds
        .expect("a rule fired this bucket — must be Some");
    assert_eq!(
        rule_match_seconds.get("rest").and_then(|m| m.get("url:youtube")),
        Some(&2.0),
        "must attribute exactly the ruled-on seconds to the specific rule key that fired"
    );
    assert!(
        !rule_match_seconds.contains_key("browser"),
        "no rule fired for the plain \"browser\" tick — must not appear in rule_match_seconds at all"
    );
}

#[test]
fn rule_match_seconds_is_none_when_no_rule_ever_fires_in_the_bucket() {
    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    acc.accumulate(&Tick {
        active_process_name: Some("code.exe".to_string()),
        keyboard_events: 0,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        matched_rule_key: None,
        occurred_at: base_time(),
        interval_seconds: 2.0,
    });
    let signals = acc.flush(None);
    assert_eq!(
        signals.rule_match_seconds,
        Some(BTreeMap::new()),
        "app_detail consent is on, so this is Some — just an empty map, not None, matching \
         category_app_seconds/app_seconds's own \"measured, found nothing\" convention"
    );
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
        matched_rule_key: None,
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
        matched_rule_key: None,
        occurred_at: base_time(),
        interval_seconds: 2.0,
    });
    let signals = acc.flush(None);
    assert_eq!(signals.idle_seconds, Some(0.0));
}

// --- "Сон/выключение не растягивает открытый сегмент" (реальный баг,
// найденный на скриншоте пользователя: "Браузер" 00:38-09:21 одним
// сплошным сегментом, хотя компьютер явно не работал бóльшую часть
// этого времени — accumulate() просто не вызывался, пока агент спал,
// и когда он проснулся, close_current_segment посчитал длительность
// как "сейчас минус когда сегмент открылся", не заметив реальный
// разрыв между последним и следующим тиком). ---

#[test]
fn a_long_real_gap_between_ticks_closes_the_open_segment_at_the_last_real_tick_not_now() {
    let opened_at = base_time();
    // Нормальный шаг опроса (2с) — не должен сам по себе выглядеть как
    // разрыв; реальный разрыв ниже — единственный намеренно большой.
    let last_seen = opened_at + chrono::Duration::seconds(2);
    let woke_up = last_seen + chrono::Duration::hours(8) + chrono::Duration::minutes(43);

    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    // Открывает сегмент.
    acc.accumulate(&Tick {
        active_process_name: Some("chrome.exe".to_string()),
        keyboard_events: 1,
        mouse_move_events: 1,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        matched_rule_key: None,
        occurred_at: opened_at,
        interval_seconds: 2.0,
    });
    // Последний реальный тик перед сном — та же категория, сегмент
    // остаётся открытым (started_at не сдвигается).
    acc.accumulate(&Tick {
        active_process_name: Some("chrome.exe".to_string()),
        keyboard_events: 1,
        mouse_move_events: 1,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        matched_rule_key: None,
        occurred_at: last_seen,
        interval_seconds: 2.0,
    });
    // Компьютер спал 8ч43м — следующий тик агент видит только сейчас,
    // с тем же самым процессом на переднем плане (никакой смены
    // категории, которая иначе закрыла бы сегмент сама).
    acc.accumulate(&Tick {
        active_process_name: Some("chrome.exe".to_string()),
        keyboard_events: 1,
        mouse_move_events: 1,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        matched_rule_key: None,
        occurred_at: woke_up,
        interval_seconds: 2.0,
    });
    // Ещё один тик другой категории, чтобы закрыть сегмент, открытый
    // на пробуждении, и реально увидеть его в flush() (open-сегмент,
    // как и открытый gap, сам по себе не появляется в closed_segments
    // до тех пор, пока его что-то не закроет — то же самое верно и без
    // этого бага, независимая от него часть контракта).
    acc.accumulate(&Tick {
        active_process_name: Some("code.exe".to_string()),
        keyboard_events: 1,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        matched_rule_key: None,
        occurred_at: woke_up + chrono::Duration::seconds(2),
        interval_seconds: 2.0,
    });

    let signals = acc.flush(None);
    let segments = signals.activity_segments.unwrap();

    // Первый сегмент закрылся РОВНО на последнем реальном тике — не
    // растянулся на все 8ч43м простоя.
    let first = segments
        .iter()
        .find(|s| s.started_at == opened_at)
        .expect("segment starting at opened_at must exist");
    assert_eq!(
        first.ended_at, last_seen,
        "closed at the last real tick, not stretched to the wake-up time"
    );
    assert!(
        (first.duration_seconds - 2.0).abs() < 0.01,
        "duration must reflect only real observed time (~2s), not the 8h43m gap: got {}",
        first.duration_seconds
    );

    // Второй сегмент начался заново с момента пробуждения — не продолжение первого.
    let second = segments
        .iter()
        .find(|s| s.started_at == woke_up)
        .expect("a fresh segment starting at wake-up must exist");
    assert_eq!(second.category, "browser");

    // Сам простой — необъяснённый провал (порог 900с уже пройден: 8ч43м).
    let gaps = signals.unexplained_gaps.unwrap();
    let sleep_gap = gaps
        .iter()
        .find(|g| g.started_at == last_seen && g.ended_at == woke_up)
        .expect("the sleep interval itself must be recorded as an unexplained gap");
    assert!((sleep_gap.duration_seconds - 31380.0).abs() < 1.0); // 8h43m in seconds
}

#[test]
fn a_short_gap_between_ticks_does_not_split_the_segment() {
    // Убеждаемся, что порог не слишком чувствительный — обычная
    // задержка планировщика (несколько секунд, не 8 часов) не должна
    // ложно резать нормальный, непрерывный сегмент на два.
    let t0 = base_time();
    let t1 = t0 + chrono::Duration::seconds(5); // нормальный джиттер, не разрыв

    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    acc.accumulate(&Tick {
        active_process_name: Some("code.exe".to_string()),
        keyboard_events: 1,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        matched_rule_key: None,
        occurred_at: t0,
        interval_seconds: 2.0,
    });
    acc.accumulate(&Tick {
        active_process_name: Some("code.exe".to_string()),
        keyboard_events: 1,
        mouse_move_events: 0,
        mouse_click_events: 0,
        is_idle: false,
        category_override: None,
        matched_rule_key: None,
        occurred_at: t1,
        interval_seconds: 2.0,
    });
    // По самой категории сегмент не рвётся (та же "ide" всё время,
    // никакого форс-закрытия по короткому джиттеру между тиками) — но
    // flush() теперь ВСЕГДА раскрывает то, что накопилось в открытом
    // сегменте на момент своего вызова (см. flush()'а докстринг: баг с
    // пользовательского скриншота, где "Лента дня" была пустой весь
    // долгий одноприложенческий отрезок и получала один огромный
    // сегмент только на переключении). Поэтому здесь ровно один сегмент
    // длиной в реальный прошедший интервал (t0..t1, 5с), не 0 — но
    // именно ОДИН, не два: короткий джиттер между тиками сам по себе
    // сегмент не режет, это по-прежнему проверяется через gaps ниже.
    let signals = acc.flush(None);
    let segments = signals.activity_segments.unwrap();
    assert_eq!(
        segments.len(),
        1,
        "flush() splits the still-open segment at its own boundary, but a short scheduling delay must not ALSO force-close it via the gap heuristic (that would show up as unexplained_gaps below, or a second segment)"
    );
    assert_eq!(segments[0].category, "ide");
    assert_eq!(segments[0].started_at, t0);
    assert_eq!(segments[0].ended_at, t1);
    assert_eq!(signals.unexplained_gaps.unwrap().len(), 0);
}

// --- "Долгое пребывание в одной категории видно по частям, не одним
//     ретроактивным куском" (реальный баг, data_bugs/05: "Лента дня"
//     показывала пустоту, пока человек не переключался, а потом сразу
//     весь накопленный отрезок одним сегментом) ---

#[test]
fn a_long_same_category_dwell_is_split_across_multiple_flushes_not_one_retroactive_block() {
    // Реалистичная плотность тиков (POLL_INTERVAL в main.rs — 2с), гэп
    // между соседними тиками всегда меньше MAX_TICK_GAP_SECONDS (30с),
    // чтобы не задеть сон/простой-эвристику — тестируем именно
    // flush()'а границу (EXPORT_INTERVAL_SECONDS в main.rs — 60с), не
    // разрыв между тиками.
    let t0 = base_time();
    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);
    let mut now = t0;

    let tick_same_category = |acc: &mut BucketAccumulator, at: DateTime<Utc>| {
        acc.accumulate(&Tick {
            active_process_name: Some("code.exe".to_string()),
            keyboard_events: 1,
            mouse_move_events: 0,
            mouse_click_events: 0,
            is_idle: false,
            category_override: None,
            matched_rule_key: None,
            occurred_at: at,
            interval_seconds: 2.0,
        });
    };

    // Открываем сегмент и держим его 60 секунд одними и теми же тиками
    // (30 штук по 2с) — ни одного переключения категории.
    tick_same_category(&mut acc, now);
    for _ in 0..29 {
        now += chrono::Duration::seconds(2);
        tick_same_category(&mut acc, now);
    }
    let t_flush_1 = now; // t0 + 58s

    let signals_1 = acc.flush(None);
    let segments_1 = signals_1.activity_segments.unwrap();
    assert_eq!(
        segments_1.len(),
        1,
        "flush() must surface its own slice of the still-open segment, not stay empty until a category change ever happens"
    );
    assert_eq!(segments_1[0].started_at, t0);
    assert_eq!(segments_1[0].ended_at, t_flush_1);

    // Ещё 60 секунд той же самой категории, затем второй flush().
    for _ in 0..30 {
        now += chrono::Duration::seconds(2);
        tick_same_category(&mut acc, now);
    }
    let t_flush_2 = now;

    let signals_2 = acc.flush(None);
    let segments_2 = signals_2.activity_segments.unwrap();
    assert_eq!(
        segments_2.len(),
        1,
        "the second flush must ALSO surface its own slice — not stay empty until a category change ever happens"
    );
    assert_eq!(
        segments_2[0].started_at, t_flush_1,
        "picks up exactly where the previous flush's slice left off, no gap and no overlap"
    );
    assert_eq!(segments_2[0].ended_at, t_flush_2);
}

// --- "Manual category override applies to future ticks, never retroactively" ---
// Real user report: an unrecognized process name (e.g. a Telegram Desktop
// variant) falls into "other" under the built-in map; the dashboard's
// manual override (see agent-bin's run_category_overrides_loop) must
// change how it's categorized going forward without altering anything
// already accumulated before the override was applied.

#[test]
fn set_category_overrides_affects_only_ticks_accumulated_afterward() {
    let t0 = base_time();
    let mut acc = BucketAccumulator::new(full_consent(), BTreeMap::new(), 900.0);

    let tick = |acc: &mut BucketAccumulator, at: DateTime<Utc>| {
        acc.accumulate(&Tick {
            active_process_name: Some("unrecognized-app.exe".to_string()),
            keyboard_events: 1,
            mouse_move_events: 0,
            mouse_click_events: 0,
            is_idle: false,
            category_override: None,
            matched_rule_key: None,
            occurred_at: at,
            interval_seconds: 2.0,
        });
    };

    // Before any override: falls into "other", per the built-in map.
    tick(&mut acc, t0);
    let mut overrides = BTreeMap::new();
    overrides.insert("unrecognized-app.exe".to_string(), "communication".to_string());
    acc.set_category_overrides(overrides);

    // After the override: the same process name now resolves differently.
    tick(&mut acc, t0 + chrono::Duration::seconds(2));

    let signals = acc.flush(None);
    let category_seconds = signals.active_app_category_seconds.unwrap();
    assert_eq!(
        category_seconds.get("other").copied().unwrap_or(0.0),
        2.0,
        "the tick accumulated BEFORE the override must keep its original category"
    );
    assert_eq!(
        category_seconds.get("communication").copied().unwrap_or(0.0),
        2.0,
        "the tick accumulated AFTER the override must use the new category"
    );
}
