//! Mirrors `backend/app/models.py` (`Signals`, `Consent`, `ActivitySegment`,
//! `UnexplainedGap`, `InputActivityEvents`, and the `period_start`/
//! `period_end`/`signature` fields of `ProductivityRecordIn`) field-for-field.
//! Field NAMES here are load-bearing: the real backend already has historical
//! `ProductivityRecordORM.payload` rows keyed by these exact names, and every
//! dashboard page (Today/Trend/Patterns/History/Sessions/Reviews/Goals) reads
//! them directly (see `AGENT_EVENT_PARITY.md` §4). Renaming any of them here
//! would silently break every one of those pages for historical data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InputActivityEvents {
    pub keyboard: i64,
    pub mouse_move: i64,
    pub mouse_click: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivitySegment {
    pub category: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnexplainedGap {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Signals {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_app_category_seconds: Option<BTreeMap<String, f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_activity_events: Option<InputActivityEvents>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_seconds: Option<f64>,
    /// The one field the current backend requires unconditionally (not
    /// consent-gated at the model level) — see `AGENT_EVENT_PARITY.md` §1.
    pub active_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_segments: Option<Vec<ActivitySegment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unexplained_gaps: Option<Vec<UnexplainedGap>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commits_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_seconds: Option<BTreeMap<String, f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_app_seconds: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Consent {
    pub active_app_category: bool,
    pub input_activity_counts: bool,
    pub idle_tracking: bool,
    #[serde(default)]
    pub activity_segments: bool,
    #[serde(default)]
    pub unexplained_gaps: bool,
    #[serde(default)]
    pub git_activity: bool,
    #[serde(default)]
    pub app_detail: bool,
}

/// The part of a record that is genuinely payload (as opposed to envelope
/// metadata) — the current MVP schema's entire useful content, unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub signals: Signals,
    pub consent: Consent,
    #[serde(default)]
    pub signature: Option<String>,
}
