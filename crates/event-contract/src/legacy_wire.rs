use crate::envelope::{AggregationLevel, Envelope, NormalizedType, PrivacyClass, SourceType};
use crate::payload::{Consent, Signals};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// The exact wire shape `backend/app/models.py::ProductivityRecordIn` expects
/// today. `user_id` is included purely for backward shape-compatibility: the
/// real backend already resolves identity from `X-Agent-Token`
/// (`backend/app/routes/ingest.py::verify_agent_token`) and ignores this
/// field's value, so any non-empty string satisfies the model — this crate
/// passes the envelope's own `device_id` there rather than inventing a
/// separate placeholder.
///
/// Envelope-only fields (`event_id`, `envelope_version`, `device_id`,
/// `timezone_offset`, `source_type`, `normalized_type`, `privacy_class`,
/// `aggregation_level`) ride along as additional top-level JSON keys.
/// `ProductivityRecordIn` has no `model_config` overriding Pydantic v2's
/// default `extra="ignore"` (confirmed by reading `backend/app/models.py`),
/// so today's unmodified backend accepts this body unchanged, silently
/// dropping the new keys — this is the "migration adapter" required by
/// `CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md`'s AG-003 acceptance
/// criteria, verified against a real backend in this crate's tests, not
/// just asserted from reading the model source.
#[derive(Debug, Clone, Serialize)]
pub struct LegacyWireRecord<'a> {
    pub schema_version: &'a str,
    pub agent_version: &'a str,
    pub user_id: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub signals: &'a Signals,
    pub consent: &'a Consent,
    pub signature: &'a Option<String>,

    pub event_id: String,
    pub envelope_version: &'a str,
    pub device_id: String,
    pub timezone_offset: &'a str,
    pub source_type: SourceType,
    pub normalized_type: NormalizedType,
    pub privacy_class: PrivacyClass,
    pub aggregation_level: AggregationLevel,
}

impl Envelope {
    pub fn to_legacy_wire(&self) -> LegacyWireRecord<'_> {
        LegacyWireRecord {
            schema_version: &self.schema_version,
            agent_version: &self.agent_version,
            user_id: self.device_id.to_string(),
            period_start: self.payload.period_start,
            period_end: self.payload.period_end,
            signals: &self.payload.signals,
            consent: &self.payload.consent,
            signature: &self.payload.signature,
            event_id: self.event_id.to_string(),
            envelope_version: &self.envelope_version,
            device_id: self.device_id.to_string(),
            timezone_offset: &self.timezone_offset,
            source_type: self.source_type,
            normalized_type: self.normalized_type,
            privacy_class: self.privacy_class,
            aggregation_level: self.aggregation_level,
        }
    }
}
