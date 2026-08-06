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
    /// Schema 0.6.0-prototype — ground truth "which app contributed how
    /// much to which resolved category," computed by the agent itself at
    /// collection time (not re-derived by the backend from `app_seconds`
    /// alone, which can't know a title/URL rule reclassified part of one
    /// app's time). `None` for records from an agent older than 0.6.0 —
    /// the backend falls back to its pre-existing `app_seconds`-based
    /// re-derivation for those, unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_app_seconds: Option<BTreeMap<String, BTreeMap<String, f64>>>,
    /// Schema 0.6.0-prototype — time attributed specifically to a fired
    /// title/URL rule, nested by resolved category, then by WHICH APP the
    /// rule fired for, then by a `"title:<keyword>"`/`"url:<keyword>"`
    /// key — the app-nesting level is what lets a consumer tell "this
    /// app's time in this category came from this specific rule" apart
    /// from other, unrelated apps/rules in the same category (a flat
    /// category->rule map, this field's first cut, couldn't distinguish
    /// that — found from a real, confusing double display it produced).
    /// `None` for pre-0.6.0 agents or buckets where no rule fired at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_match_seconds: Option<BTreeMap<String, BTreeMap<String, BTreeMap<String, f64>>>>,
    /// Schema 0.7.0-prototype (Company Layer) — SECOND, independent
    /// categorization channel, computed ONLY from company/department
    /// title/URL rules (see `agent-bin`'s `run_company_title_rules_loop`/
    /// `run_company_url_rules_loop`), never from this user's own personal
    /// rules or `active_app_category_seconds` above. Exists so a
    /// company's group aggregate is comparable across every employee —
    /// each one is bucketed by the exact same rule set, regardless of
    /// what personal rules any individual has configured for their own
    /// view. `None` when the user isn't in an active company, or their
    /// agent predates this field. Never surfaced in this employee's own
    /// Today/History — only fed into the company/department aggregate
    /// (see backend's `company_aggregation.py`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_category_seconds: Option<BTreeMap<String, f64>>,
    /// Schema 0.7.0-prototype — opt-in personal tally of time spent per
    /// browser domain (bare host only, via `url_classifier::extract_host`
    /// — never path/query/title). Gated by a NEW, standalone consent
    /// purpose (`domain_tracking`, backend's
    /// `consent.py:CONSENT_PURPOSE_DOMAIN_TRACKING`) polled from
    /// `GET /v1/agent/domain-tracking` (see `agent-bin`'s
    /// `run_domain_tracking_poll_loop`) — deliberately NOT gated by
    /// `Consent.active_app_category` the way `company_category_seconds`
    /// is, since this is a fully separate, riskier opt-in the user must
    /// accept explicitly (see backend's consent text). `None` when the
    /// toggle is off, or the user hasn't enabled it. Personal channel
    /// only — never fed into any company/team aggregate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_seconds: Option<BTreeMap<String, f64>>,
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
    /// Echo of "domain tracking was enabled for this tick's bucket" —
    /// audit-trail field, same role as `app_detail` above, not the
    /// enforcement mechanism itself (that's the `GET
    /// /v1/agent/domain-tracking` poll — see `Signals::domain_seconds`).
    #[serde(default)]
    pub domain_tracking: bool,
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
