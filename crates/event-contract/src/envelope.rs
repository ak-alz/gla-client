use crate::ids::{DeviceId, EventId};
use crate::payload::Payload;
use crate::quarantine::QuarantinedEvent;
use crate::validation::{validate, ContractViolation};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The wire `schema_version` value this crate emits. Pinned to the value the
/// current, unmodified backend already accepts (`AGENT_EVENT_PARITY.md` §1) —
/// this crate adds envelope metadata as ADDITIVE fields (see `legacy_wire`),
/// it does not change what `schema_version` itself means on the wire. Bumping
/// this is a backend-coordinated change, out of this crate's scope.
pub const SCHEMA_VERSION: &str = "0.5.0-prototype";

/// Versions the shape introduced by THIS crate (which envelope fields exist,
/// their types) — independent of `SCHEMA_VERSION` above, which the legacy
/// backend wire format owns. Bump this when `Envelope`'s own fields change.
pub const ENVELOPE_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    /// The only source the current MVP has: a fixed-interval poll/bucket
    /// loop reading OS-level active-window/idle/input signals.
    DesktopPeriodicCollector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedType {
    /// The only record shape the current MVP produces — a closed period's
    /// aggregated signals, never a single raw event.
    ProductivityAggregate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyClass {
    /// Counts, durations, and category labels only — never raw window text,
    /// keystrokes, or screen content (see `title_classifier.py`'s "classify
    /// then discard" behavior in the Python MVP, preserved as an invariant
    /// here, not merely a code-review convention as `AGENT_EVENT_PARITY.md`
    /// §3 notes it is today).
    AggregatedBehavioral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregationLevel {
    /// One flush of the collector's poll/bucket loop — today's only
    /// granularity (no raw per-event or daily-rollup records exist yet).
    Bucket,
}

/// The versioned event envelope: the target shape from
/// `docs/02_ARCHITECTURE/AGENT_ARCHITECTURE.md`'s "Event envelope" section,
/// wrapping the existing `Payload` (itself unchanged from the Python MVP)
/// with the metadata that section requires and the current schema is
/// missing (see `AGENT_EVENT_PARITY.md` §3: 6 of 11 fields were absent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub event_id: EventId,
    pub envelope_version: String,
    pub schema_version: String,
    pub device_id: DeviceId,
    pub agent_version: String,
    pub occurred_at: DateTime<Utc>,
    pub timezone_offset: String,
    pub source_type: SourceType,
    pub normalized_type: NormalizedType,
    pub privacy_class: PrivacyClass,
    pub aggregation_level: AggregationLevel,
    pub payload: Payload,
}

/// Inputs the CALLER must supply — everything else (`event_id`, versions,
/// `occurred_at`, `timezone_offset`, and the fixed classification fields) is
/// derived by `Envelope::try_new`/`build_or_quarantine`.
pub struct NewEnvelope {
    pub device_id: DeviceId,
    pub agent_version: String,
    pub payload: Payload,
}

/// Formats the current local UTC offset as `+HH:MM`/`-HH:MM`. Computed at
/// construction time (not cached) so it is correct across a DST transition
/// spanning multiple envelope constructions in a long-running process.
pub fn current_timezone_offset() -> String {
    format_offset_seconds(chrono::Local::now().offset().local_minus_utc())
}

fn format_offset_seconds(total_seconds: i32) -> String {
    let sign = if total_seconds < 0 { '-' } else { '+' };
    let abs = total_seconds.unsigned_abs();
    let hours = abs / 3600;
    let minutes = (abs % 3600) / 60;
    format!("{sign}{hours:02}:{minutes:02}")
}

impl Envelope {
    /// Validates `input.payload` and constructs an envelope, or returns
    /// every `ContractViolation` found. Prefer `build_or_quarantine` unless
    /// the caller needs the violations in a form other than a quarantine
    /// record (e.g. surfacing them in a diagnostics UI).
    pub fn try_new(input: NewEnvelope) -> Result<Self, Vec<ContractViolation>> {
        let violations = validate(&input.payload);
        if !violations.is_empty() {
            return Err(violations);
        }

        Ok(Envelope {
            event_id: EventId::new(),
            envelope_version: ENVELOPE_VERSION.to_string(),
            schema_version: SCHEMA_VERSION.to_string(),
            device_id: input.device_id,
            agent_version: input.agent_version,
            // For a period-aggregate, "when this occurred" is best represented
            // by when the period closed (the moment the record became final) —
            // this mirrors the same choice `backend/app/routes/ingest.py`
            // already makes for its clock-skew check (compares server time
            // against `period_end`, not `period_start`).
            occurred_at: input.payload.period_end,
            timezone_offset: current_timezone_offset(),
            source_type: SourceType::DesktopPeriodicCollector,
            normalized_type: NormalizedType::ProductivityAggregate,
            privacy_class: PrivacyClass::AggregatedBehavioral,
            aggregation_level: AggregationLevel::Bucket,
            payload: input.payload,
        })
    }

    /// Like `try_new`, but on failure returns a `QuarantinedEvent` carrying
    /// the raw payload (serialized independently of the failed validation)
    /// plus every violation found — satisfying "invalid events quarantined"
    /// rather than merely rejected-and-forgotten.
    pub fn build_or_quarantine(input: NewEnvelope) -> Result<Self, QuarantinedEvent> {
        let raw_payload = serde_json::to_value(&input.payload)
            .expect("Payload serialization is infallible: no non-finite floats are constructed by this crate, and all other fields are plain owned data");
        let payload_for_validation = input.payload.clone();
        match Self::try_new(input) {
            Ok(envelope) => Ok(envelope),
            Err(_) => {
                let violations = validate(&payload_for_validation);
                Err(QuarantinedEvent::new(raw_payload, violations))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_formatting_handles_sign_and_padding() {
        assert_eq!(format_offset_seconds(0), "+00:00");
        assert_eq!(format_offset_seconds(5 * 3600), "+05:00");
        assert_eq!(format_offset_seconds(-5 * 3600 - 30 * 60), "-05:30");
        assert_eq!(format_offset_seconds(9 * 3600 + 30 * 60), "+09:30");
    }
}
