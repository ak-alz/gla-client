use crate::backoff::{compute_backoff, BackoffConfig};
use crate::transport::{TransportOutcome, UploadTransport};
use durable_queue::{DurableQueue, QueueError};
use std::time::Duration;

pub struct UploaderConfig {
    pub batch_size: usize,
    pub backoff: BackoffConfig,
}

/// Consecutive-failure counter driving backoff growth. Reset to zero on any
/// success. Deliberately in-memory only (not persisted): the durable queue
/// (AG-004) already guarantees no data is lost across a restart, and
/// starting backoff fresh after a restart is the conservative choice — if
/// the network is genuinely still down, the very next failed attempt
/// re-grows it immediately, so this costs at most one optimistic retry, not
/// a sustained storm.
#[derive(Debug, Default)]
pub struct BackoffState {
    consecutive_failures: u32,
}

impl BackoffState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BackoffReason {
    RateLimited,
    ServerError,
    NetworkError,
}

#[derive(Debug, PartialEq)]
pub enum CycleOutcome {
    /// Nothing was pending to upload.
    Idle,
    /// Uploaded `uploaded` records; `client_errors` records in this batch
    /// were rejected by the server for reasons specific to their own
    /// content (not the server's health) and were left un-acked, not
    /// retried in this cycle.
    Progress {
        uploaded: usize,
        client_errors: usize,
    },
    /// Stop uploading for `after` — a transient, server-or-network-wide
    /// condition. The caller should wait at least `after` before calling
    /// `run_once` again.
    Backoff {
        after: Duration,
        reason: BackoffReason,
    },
    /// The agent token itself is invalid — retrying will never succeed;
    /// the caller must trigger re-pairing (out of this crate's scope),
    /// not back off and retry the same token.
    Unauthorized,
}

pub struct Uploader<'a, T: UploadTransport> {
    transport: &'a T,
    config: UploaderConfig,
}

impl<'a, T: UploadTransport> Uploader<'a, T> {
    pub fn new(transport: &'a T, config: UploaderConfig) -> Self {
        Uploader { transport, config }
    }

    /// Drains up to `config.batch_size` pending records from `queue`,
    /// uploading each in turn. Stops at the FIRST server-or-network-wide
    /// failure in the batch (`RateLimited`/`ServerError`/`NetworkError`/
    /// `Unauthorized`) rather than continuing to hammer the remaining
    /// records against a backend that just failed — this is what "no retry
    /// storm" means at the batch level; the `Duration` in
    /// `CycleOutcome::Backoff` is the caller's signal for how long to wait
    /// before calling this again.
    ///
    /// A per-record `ClientError` (any 4xx other than 401/429) is different:
    /// it is specific to that one record's content, not the server's
    /// health, so it does NOT stop the batch — the next record's content is
    /// unrelated and may well succeed. The rejected record is left un-acked
    /// (so it is inspectable, not silently discarded) but is not retried
    /// within this cycle either; it will be attempted again on the next
    /// drain, once per cycle rather than in a tight loop. A record that
    /// keeps failing this way forever (a "poison pill") is a known,
    /// documented limitation of this task — see SUMMARY.md — not something
    /// this crate builds a dead-letter mechanism for.
    pub fn run_once(&self, queue: &DurableQueue, state: &mut BackoffState) -> CycleOutcome {
        let batch = match queue.dequeue_batch(self.config.batch_size) {
            Ok(batch) => batch,
            // A local queue I/O error means the server was never contacted —
            // there is nothing to back off against, and nothing safe to
            // upload right now either.
            Err(
                QueueError::Io(_)
                | QueueError::UnsupportedFormatVersion { .. }
                | QueueError::AlreadyOpen { .. }
                | QueueError::Full { .. },
            ) => return CycleOutcome::Idle,
        };
        if batch.is_empty() {
            return CycleOutcome::Idle;
        }

        let mut uploaded = 0;
        let mut client_errors = 0;
        for (index, record) in batch.iter().enumerate() {
            let body = serde_json::to_vec(&record.envelope.to_legacy_wire())
                .expect("LegacyWireRecord serialization is infallible: no non-finite floats reach this point (event-contract's validate() already rejects them)");

            match self.transport.post_record(&body) {
                TransportOutcome::Success { .. } => {
                    // Idempotency (client side): if this ack's response is
                    // itself lost, the record simply stays leased and gets
                    // released back to pending (see below) or recovered on
                    // the next restart, then re-uploaded — the backend does
                    // not yet dedupe by event_id on receipt (a known,
                    // documented cross-project gap, see SUMMARY.md), so a
                    // lost ack-of-a-successful-upload can still produce a
                    // duplicate row server-side. Closing that fully needs a
                    // backend change, explicitly out of this task's scope.
                    let _ = queue.ack(&record.envelope.event_id);
                    uploaded += 1;
                    state.consecutive_failures = 0;
                }
                TransportOutcome::Unauthorized => {
                    self.release_remaining(queue, &batch, index);
                    return CycleOutcome::Unauthorized;
                }
                TransportOutcome::RateLimited { retry_after } => {
                    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                    let after = retry_after.unwrap_or_else(|| {
                        compute_backoff(&self.config.backoff, state.consecutive_failures)
                    });
                    self.release_remaining(queue, &batch, index);
                    return CycleOutcome::Backoff {
                        after,
                        reason: BackoffReason::RateLimited,
                    };
                }
                TransportOutcome::ServerError { .. } => {
                    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                    self.release_remaining(queue, &batch, index);
                    return CycleOutcome::Backoff {
                        after: compute_backoff(&self.config.backoff, state.consecutive_failures),
                        reason: BackoffReason::ServerError,
                    };
                }
                TransportOutcome::NetworkError => {
                    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                    self.release_remaining(queue, &batch, index);
                    return CycleOutcome::Backoff {
                        after: compute_backoff(&self.config.backoff, state.consecutive_failures),
                        reason: BackoffReason::NetworkError,
                    };
                }
                TransportOutcome::ClientError { .. } => {
                    // Specific to this record, not the batch — released
                    // immediately (not left leased) so it is visible again
                    // for the next drain cycle, but the loop continues
                    // rather than bailing out, per this branch's contract
                    // (see the doc comment above).
                    let _ = queue.release(&record.envelope.event_id);
                    client_errors += 1;
                }
            }
        }

        CycleOutcome::Progress {
            uploaded,
            client_errors,
        }
    }

    /// Releases every batch record from `from_index` onward back to
    /// `pending/` — used whenever `run_once` bails out of a batch early
    /// (a server/network/auth failure): the record that just failed AND
    /// every record after it in this batch were dequeued (moved to
    /// `leased/`) upfront by the single `dequeue_batch` call above, so all
    /// of them — not just the one that failed — would otherwise be
    /// stranded in `leased/`, invisible to the next `run_once` call's own
    /// `dequeue_batch`, until the whole process restarted.
    fn release_remaining(
        &self,
        queue: &DurableQueue,
        batch: &[durable_queue::QueuedRecord],
        from_index: usize,
    ) {
        for record in &batch[from_index..] {
            let _ = queue.release(&record.envelope.event_id);
        }
    }
}
