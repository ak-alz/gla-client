//! Contract tests shared across every platform (this crate has no
//! `cfg(target_os = ...)` code at all, so these tests exercise the exact
//! same logic every collector will use). Covers every AG-003 acceptance
//! criterion: legacy-backend wire compatibility, Python-MVP field parity,
//! forward-compatible unknown fields, and quarantine of invalid events.

use chrono::{DateTime, Duration, Utc};
use event_contract::{
    ContractViolation, DeviceId, Envelope, InputActivityEvents, NewEnvelope, Payload, Signals,
};
use std::collections::BTreeMap;
use uuid::Uuid;

fn base_period() -> (DateTime<Utc>, DateTime<Utc>) {
    let end: DateTime<Utc> = "2026-07-17T12:00:00Z".parse().unwrap();
    let start = end - Duration::seconds(120);
    (start, end)
}

fn valid_payload() -> Payload {
    let (period_start, period_end) = base_period();
    let mut active_app_category_seconds = BTreeMap::new();
    active_app_category_seconds.insert("deep_work".to_string(), 90.0);
    active_app_category_seconds.insert("communication".to_string(), 30.0);

    Payload {
        period_start,
        period_end,
        signals: Signals {
            active_app_category_seconds: Some(active_app_category_seconds),
            input_activity_events: Some(InputActivityEvents {
                keyboard: 240,
                mouse_move: 80,
                mouse_click: 12,
            }),
            idle_seconds: Some(0.0),
            active_seconds: 120.0,
            activity_segments: None,
            unexplained_gaps: None,
            git_commits_count: None,
            app_seconds: None,
            other_app_seconds: None,
        },
        consent: event_contract::Consent {
            active_app_category: true,
            input_activity_counts: true,
            idle_tracking: true,
            activity_segments: false,
            unexplained_gaps: false,
            git_activity: false,
            app_detail: false,
        },
        signature: None,
    }
}

fn device_id() -> DeviceId {
    DeviceId::from_uuid(Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap())
}

fn build_valid_envelope() -> Envelope {
    Envelope::try_new(NewEnvelope {
        device_id: device_id(),
        agent_version: "0.1.0-rust".to_string(),
        payload: valid_payload(),
    })
    .expect("valid payload must construct an envelope")
}

// --- AG-003: "Совместимо с Python MVP output либо есть migration adapter" ---

#[test]
fn legacy_wire_preserves_every_python_mvp_field_name_and_type() {
    let envelope = build_valid_envelope();
    let wire = serde_json::to_value(envelope.to_legacy_wire()).unwrap();
    let obj = wire.as_object().unwrap();

    // Exactly the field set `backend/app/models.py::ProductivityRecordIn`
    // requires, byte-identical names — this IS the parity, not merely
    // similar-looking data.
    for field in [
        "schema_version",
        "agent_version",
        "user_id",
        "period_start",
        "period_end",
        "signals",
        "consent",
        "signature",
    ] {
        assert!(
            obj.contains_key(field),
            "legacy wire missing required field {field}"
        );
    }

    assert_eq!(obj["schema_version"], "0.5.0-prototype");

    let signals = obj["signals"].as_object().unwrap();
    for field in [
        "active_app_category_seconds",
        "input_activity_events",
        "idle_seconds",
        "active_seconds",
    ] {
        assert!(signals.contains_key(field), "signals missing {field}");
    }
    assert_eq!(signals["active_seconds"], 120.0);

    let consent = obj["consent"].as_object().unwrap();
    for field in [
        "active_app_category",
        "input_activity_counts",
        "idle_tracking",
        "activity_segments",
        "unexplained_gaps",
        "git_activity",
        "app_detail",
    ] {
        assert!(consent.contains_key(field), "consent missing {field}");
    }
}

#[test]
fn legacy_wire_deserializes_as_the_backend_pydantic_model_shape() {
    // A hand-maintained mirror of `backend/app/models.py::ProductivityRecordIn`
    // — if this crate's output stops matching that model's required fields,
    // this struct (not a live backend call) catches it immediately, in every
    // environment, with no server needed.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct ProductivityRecordInMirror {
        schema_version: String,
        agent_version: String,
        user_id: String,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        signals: SignalsMirror,
        consent: ConsentMirror,
        signature: Option<String>,
    }

    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct SignalsMirror {
        active_app_category_seconds: Option<BTreeMap<String, f64>>,
        input_activity_events: Option<InputActivityEventsMirror>,
        idle_seconds: Option<f64>,
        active_seconds: f64,
        activity_segments: Option<serde_json::Value>,
        unexplained_gaps: Option<serde_json::Value>,
        git_commits_count: Option<i64>,
        app_seconds: Option<BTreeMap<String, f64>>,
        other_app_seconds: Option<BTreeMap<String, f64>>,
    }

    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct InputActivityEventsMirror {
        keyboard: i64,
        mouse_move: i64,
        mouse_click: i64,
    }

    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct ConsentMirror {
        active_app_category: bool,
        input_activity_counts: bool,
        idle_tracking: bool,
        activity_segments: bool,
        unexplained_gaps: bool,
        git_activity: bool,
        app_detail: bool,
    }

    let envelope = build_valid_envelope();
    let json = serde_json::to_string(&envelope.to_legacy_wire()).unwrap();
    let parsed: Result<ProductivityRecordInMirror, _> = serde_json::from_str(&json);
    assert!(
        parsed.is_ok(),
        "legacy wire output does not deserialize as ProductivityRecordIn's shape: {:?}",
        parsed.err()
    );
}

// --- AG-003: "Unknown fields forward-compatible" ---

#[test]
fn legacy_wire_extra_envelope_fields_do_not_break_backend_style_strict_ignore_model() {
    // Mirrors Pydantic v2's default `extra="ignore"` behavior (confirmed by
    // reading `backend/app/models.py` — no `model_config` overrides it):
    // deserializing into a struct that only knows the legacy fields must
    // still succeed even though the JSON has additional keys
    // (`event_id`, `device_id`, `envelope_version`, `timezone_offset`,
    // `source_type`, `normalized_type`, `privacy_class`, `aggregation_level`).
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct LegacyOnly {
        schema_version: String,
        agent_version: String,
        user_id: String,
    }

    let envelope = build_valid_envelope();
    let json = serde_json::to_string(&envelope.to_legacy_wire()).unwrap();
    let parsed: Result<LegacyOnly, _> = serde_json::from_str(&json);
    assert!(
        parsed.is_ok(),
        "extra envelope fields must not break a legacy-only reader"
    );
}

#[test]
fn payload_deserialization_ignores_unknown_future_fields() {
    // Simulates a FUTURE agent version adding a field this build doesn't
    // know about yet — must still parse, not error.
    let json = serde_json::json!({
        "period_start": "2026-07-17T11:58:00Z",
        "period_end": "2026-07-17T12:00:00Z",
        "signals": { "active_seconds": 120.0 },
        "consent": {
            "active_app_category": false,
            "input_activity_counts": false,
            "idle_tracking": false
        },
        "signature": null,
        "a_field_from_the_future": { "nested": true }
    });

    let parsed: Result<Payload, _> = serde_json::from_value(json);
    assert!(
        parsed.is_ok(),
        "unknown top-level field must be forward-compatible: {:?}",
        parsed.err()
    );
}

// --- AG-003: "Invalid events quarantined" ---

#[test]
fn inverted_period_is_quarantined_not_constructed() {
    let mut payload = valid_payload();
    std::mem::swap(&mut payload.period_start, &mut payload.period_end);

    let result = Envelope::build_or_quarantine(NewEnvelope {
        device_id: device_id(),
        agent_version: "0.1.0-rust".to_string(),
        payload: payload.clone(),
    });

    let quarantined = result.expect_err("inverted period must be quarantined, not built");
    assert!(quarantined
        .violations
        .iter()
        .any(|v| v.contains("is before period_start")));
    // The raw payload must be preserved verbatim, not discarded.
    assert_eq!(quarantined.raw_payload["signals"]["active_seconds"], 120.0);
}

#[test]
fn negative_active_seconds_is_quarantined() {
    let mut payload = valid_payload();
    payload.signals.active_seconds = -1.0;

    let violations = event_contract::validate(&payload);
    assert!(violations
        .iter()
        .any(|v| matches!(v, ContractViolation::NegativeActiveSeconds(_))));
}

#[test]
fn nan_active_seconds_is_quarantined() {
    // NaN compares false to every ordering (`NaN < 0.0` is false), so a
    // plain negativity check alone would miss it entirely — and a non-finite
    // float silently serializes to JSON `null`, which would fail the real
    // backend's required `active_seconds` field far downstream instead of
    // being caught here.
    let mut payload = valid_payload();
    payload.signals.active_seconds = f64::NAN;

    let violations = event_contract::validate(&payload);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, ContractViolation::NonFiniteActiveSeconds(_))),
        "expected a NonFiniteActiveSeconds violation, got: {violations:?}"
    );

    let result = Envelope::build_or_quarantine(NewEnvelope {
        device_id: device_id(),
        agent_version: "0.1.0-rust".to_string(),
        payload,
    });
    assert!(
        result.is_err(),
        "NaN active_seconds must be quarantined, not built"
    );
}

#[test]
fn infinite_idle_seconds_is_quarantined() {
    let mut payload = valid_payload();
    payload.signals.idle_seconds = Some(f64::INFINITY);

    let violations = event_contract::validate(&payload);
    assert!(violations
        .iter()
        .any(|v| matches!(v, ContractViolation::NonFiniteIdleSeconds(_))));
}

#[test]
fn non_finite_category_seconds_is_quarantined() {
    let mut payload = valid_payload();
    let mut map = BTreeMap::new();
    map.insert("deep_work".to_string(), f64::NAN);
    payload.signals.active_app_category_seconds = Some(map);

    let violations = event_contract::validate(&payload);
    assert!(violations
        .iter()
        .any(|v| matches!(v, ContractViolation::NonFiniteCategorySeconds { .. })));
}

#[test]
fn data_present_without_matching_consent_is_quarantined() {
    let mut payload = valid_payload();
    payload.consent.active_app_category = false; // data is present (see valid_payload) but consent says no

    let violations = event_contract::validate(&payload);
    assert!(
        violations.iter().any(|v| matches!(
            v,
            ContractViolation::MissingConsent {
                field: "signals.active_app_category_seconds",
                ..
            }
        )),
        "expected a MissingConsent violation, got: {violations:?}"
    );
}

#[test]
fn empty_category_name_is_quarantined() {
    let mut payload = valid_payload();
    let mut map = BTreeMap::new();
    map.insert("".to_string(), 10.0);
    payload.signals.active_app_category_seconds = Some(map);

    let violations = event_contract::validate(&payload);
    assert!(violations
        .iter()
        .any(|v| matches!(v, ContractViolation::EmptyCategoryName { .. })));
}

#[test]
fn inverted_activity_segment_is_quarantined() {
    let mut payload = valid_payload();
    payload.consent.activity_segments = true;
    let end = payload.period_end;
    let start = payload.period_start;
    payload.signals.activity_segments = Some(vec![event_contract::ActivitySegment {
        category: "deep_work".to_string(),
        started_at: end, // inverted on purpose
        ended_at: start,
        duration_seconds: 10.0,
    }]);

    let violations = event_contract::validate(&payload);
    assert!(violations
        .iter()
        .any(|v| matches!(v, ContractViolation::SegmentInverted { index: 0 })));
}

#[test]
fn valid_payload_has_zero_violations() {
    assert!(event_contract::validate(&valid_payload()).is_empty());
}

// --- General envelope contract properties ---

#[test]
fn each_envelope_gets_a_unique_event_id() {
    let a = build_valid_envelope();
    let b = build_valid_envelope();
    assert_ne!(a.event_id.to_string(), b.event_id.to_string());
}

#[test]
fn envelope_round_trips_through_json() {
    let envelope = build_valid_envelope();
    let json = serde_json::to_string(&envelope).unwrap();
    let restored: Envelope = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.event_id.to_string(), envelope.event_id.to_string());
    assert_eq!(restored.payload, envelope.payload);
}

#[test]
fn occurred_at_is_period_end_for_an_aggregate_record() {
    let envelope = build_valid_envelope();
    assert_eq!(envelope.occurred_at, envelope.payload.period_end);
}
