use crate::validation::ContractViolation;
use chrono::{DateTime, Utc};
use serde::Serialize;

/// A candidate event that failed contract validation. It is never silently
/// dropped: the raw payload and the specific reasons it failed are preserved
/// here so a future durable queue (AG-004) can persist it for inspection
/// instead of losing it outright — "invalid events quarantined", not
/// "invalid events discarded".
#[derive(Debug, Clone, Serialize)]
pub struct QuarantinedEvent {
    pub quarantined_at: DateTime<Utc>,
    pub violations: Vec<String>,
    pub raw_payload: serde_json::Value,
}

impl QuarantinedEvent {
    pub fn new(raw_payload: serde_json::Value, violations: Vec<ContractViolation>) -> Self {
        QuarantinedEvent {
            quarantined_at: Utc::now(),
            violations: violations.iter().map(ToString::to_string).collect(),
            raw_payload,
        }
    }
}
