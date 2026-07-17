//! Versioned productivity event envelope, shared by every agent-core
//! platform collector (AG-003). Wraps the existing Python-MVP-compatible
//! signals/consent payload with the metadata
//! `docs/02_ARCHITECTURE/AGENT_ARCHITECTURE.md`'s "Event envelope" section
//! requires, without renaming any existing field
//! (`docs/08_DATA/AGENT_EVENT_PARITY.md` §4). Deliberately platform-neutral:
//! no `cfg(windows)`/`cfg(target_os = ...)` anywhere in this crate, so its
//! test suite is the same "contract tests common to every platform" AG-003
//! requires, not one suite per OS.

mod envelope;
mod ids;
mod legacy_wire;
mod payload;
mod quarantine;
mod validation;

pub use envelope::{
    current_timezone_offset, AggregationLevel, Envelope, NewEnvelope, NormalizedType, PrivacyClass,
    SourceType, ENVELOPE_VERSION, SCHEMA_VERSION,
};
pub use ids::{DeviceId, EventId};
pub use legacy_wire::LegacyWireRecord;
pub use payload::{
    ActivitySegment, Consent, InputActivityEvents, Payload, Signals, UnexplainedGap,
};
pub use quarantine::QuarantinedEvent;
pub use validation::{validate, ContractViolation};
