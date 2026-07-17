//! Resilient, backoff-aware uploader (AG-005): drains `durable-queue`
//! (AG-004) and POSTs each record to the backend's existing
//! `/v1/ingest/productivity-record` endpoint via `event-contract`'s
//! (AG-003) `to_legacy_wire()`. No backend or Python-agent code is touched
//! by this crate — see `SUMMARY.md` for the explicit, user-approved scope
//! decision on end-to-end idempotency (the backend does not yet dedupe by
//! `event_id` on receipt; closing that gap is a documented follow-up, not
//! part of this task).

mod backoff;
mod transport;
mod uploader;

pub use backoff::{compute_backoff, BackoffConfig};
pub use transport::{TransportOutcome, UploadTransport, UreqTransport};
pub use uploader::{BackoffReason, BackoffState, CycleOutcome, Uploader, UploaderConfig};
