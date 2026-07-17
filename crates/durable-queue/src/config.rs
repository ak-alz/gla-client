use chrono::Duration;
use std::path::PathBuf;

/// Bounds and policy for one `DurableQueue`. Every field is required (no
/// silent defaults) — a caller must make an explicit, reviewable choice
/// about disk usage and retention rather than inherit an implicit one.
#[derive(Debug, Clone)]
pub struct QueueConfig {
    pub dir: PathBuf,
    /// Backpressure threshold on the sum of `pending/` file sizes. Enqueue
    /// rejects new records past this rather than silently dropping the
    /// oldest ones — an agent that can't reach the backend should make its
    /// caller (the uploader/collector) visibly deal with backpressure, not
    /// quietly lose data to make room.
    pub max_pending_bytes: u64,
    /// How long an ACKED record's file is kept before pruning. Never
    /// applies to `pending/` — an unacked record is retained regardless of
    /// age; only confirmed-uploaded records are ever pruned by this clock.
    pub acked_retention: Duration,
}
