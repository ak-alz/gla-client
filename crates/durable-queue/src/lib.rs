//! Crash-safe, corruption-isolating local event queue (AG-004), sitting
//! between `event-contract` (AG-003) and the future uploader (AG-005).
//! One file per record, keyed by `event_id`, in a maildir-style layout
//! (`pending/`, `leased/`, `acked/`, `quarantine/`) — chosen over a single
//! append-only log so that dedup is an O(1) existence check, ack is a
//! single rename, and one corrupt record can never block every record
//! behind it. `leased/` is the atomic "checkout" a `dequeue_batch` caller
//! moves a record into before processing it, so two concurrent callers can
//! never both receive the same record; exactly one live `DurableQueue` per
//! directory is enforced by an OS-level advisory lock (see `DurableQueue`'s
//! doc comment), not merely assumed.

mod config;
mod error;
mod queue;

pub use config::QueueConfig;
pub use error::QueueError;
pub use queue::{DurableQueue, EnqueueOutcome, QueuedRecord, QUEUE_FORMAT_VERSION};
