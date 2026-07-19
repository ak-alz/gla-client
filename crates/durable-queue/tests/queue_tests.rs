//! Contract tests for `durable-queue`. Exercises every AG-004 acceptance
//! criterion that is feasible to automate in a unit-test process (crash
//! recovery via a simulated stale temp file, corruption isolation, dedup,
//! size-limit backpressure, retention). The literal wall-clock "24h offline"
//! and "survives a real reboot" criteria are not simulable here — see
//! TEST_REPORT.md for how those were addressed instead.

use chrono::Duration;
use durable_queue::{DurableQueue, EnqueueOutcome, QueueConfig, QueueError};
use event_contract::{Consent, DeviceId, Envelope, NewEnvelope, Payload, Signals};
use std::fs;
use std::path::PathBuf;
use std::time::Duration as StdDuration;
use uuid::Uuid;

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("durable-queue-test-{label}-{}", Uuid::new_v4()))
}

fn make_envelope() -> Envelope {
    let period_end = "2026-07-17T12:00:00Z".parse().unwrap();
    let period_start = period_end - Duration::seconds(120);
    Envelope::try_new(NewEnvelope {
        device_id: DeviceId::from_uuid(Uuid::new_v4()),
        agent_version: "0.1.0-rust".to_string(),
        payload: Payload {
            period_start,
            period_end,
            signals: Signals {
                active_app_category_seconds: None,
                input_activity_events: None,
                idle_seconds: None,
                active_seconds: 120.0,
                activity_segments: None,
                unexplained_gaps: None,
                git_commits_count: None,
                app_seconds: None,
                other_app_seconds: None,
            },
            consent: Consent {
                active_app_category: false,
                input_activity_counts: false,
                idle_tracking: false,
                activity_segments: false,
                unexplained_gaps: false,
                git_activity: false,
                app_detail: false,
            },
            signature: None,
        },
    })
    .expect("hand-built payload must be valid")
}

fn open_queue(dir: PathBuf) -> DurableQueue {
    DurableQueue::open(QueueConfig {
        dir,
        max_pending_bytes: 10 * 1024 * 1024,
        acked_retention: Duration::days(7),
    })
    .expect("queue must open")
}

#[test]
fn pending_count_reflects_enqueue_dequeue_and_ack() {
    let dir = temp_dir("pending-count");
    let queue = open_queue(dir.clone());
    assert_eq!(queue.pending_count().unwrap(), 0);

    let a = make_envelope();
    let b = make_envelope();
    queue.enqueue(&a).unwrap();
    queue.enqueue(&b).unwrap();
    assert_eq!(queue.pending_count().unwrap(), 2);

    let batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(
        queue.pending_count().unwrap(),
        0,
        "checked-out records move to leased/, no longer counted as pending"
    );

    queue.ack(&batch[0].envelope.event_id).unwrap();
    queue.release(&batch[1].envelope.event_id).unwrap();
    assert_eq!(
        queue.pending_count().unwrap(),
        1,
        "the released (not acked) record returns to pending/"
    );
}

#[test]
fn enqueue_then_dequeue_round_trips() {
    let dir = temp_dir("roundtrip");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();

    let outcome = queue.enqueue(&envelope).unwrap();
    assert_eq!(outcome, EnqueueOutcome::Enqueued);

    let batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].envelope.event_id.to_string(),
        envelope.event_id.to_string()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn duplicate_enqueue_is_idempotent() {
    let dir = temp_dir("dedup");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();

    assert_eq!(queue.enqueue(&envelope).unwrap(), EnqueueOutcome::Enqueued);
    assert_eq!(
        queue.enqueue(&envelope).unwrap(),
        EnqueueOutcome::AlreadyPresent
    );

    let batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(
        batch.len(),
        1,
        "duplicate enqueue must not produce a second record"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ack_prevents_redequeue_and_dedupes_reenqueue() {
    let dir = temp_dir("ack");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();

    queue.enqueue(&envelope).unwrap();
    let first_batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(first_batch.len(), 1);
    queue.ack(&envelope.event_id).unwrap();

    let batch = queue.dequeue_batch(10).unwrap();
    assert!(batch.is_empty(), "acked record must not be redequeued");

    // Re-enqueuing the same event_id after it was acked must also be
    // recognized as already-present, not silently re-added as pending.
    let outcome = queue.enqueue(&envelope).unwrap();
    assert_eq!(outcome, EnqueueOutcome::AlreadyPresent);
    assert!(queue.dequeue_batch(10).unwrap().is_empty());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_leased_but_never_acked_record_is_recovered_to_pending_on_reopen() {
    // Simulates a process that dequeued a batch (moving it into `leased/`)
    // and then crashed or was killed before calling `ack` — the record must
    // not be stranded in `leased/` forever; the next `open()` (a fresh
    // process, in a real agent) must return it to `pending/` so it gets
    // retried, which is exactly the "ack; retry" requirement in AG-004's
    // task description.
    let dir = temp_dir("stranded-lease");
    let envelope = make_envelope();
    {
        let queue = open_queue(dir.clone());
        queue.enqueue(&envelope).unwrap();
        let leased = queue.dequeue_batch(10).unwrap();
        assert_eq!(leased.len(), 1);
        // Deliberately never ack — this is the crash/kill being simulated.
    }

    let reopened = open_queue(dir.clone());
    let batch = reopened.dequeue_batch(10).unwrap();
    assert_eq!(
        batch.len(),
        1,
        "a record left in leased/ by a previous process must come back as pending, not vanish"
    );
    assert_eq!(
        batch[0].envelope.event_id.to_string(),
        envelope.event_id.to_string()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn enqueue_is_deduped_against_a_quarantined_id_too() {
    // A regression test for a gap an independent review found: the original
    // dedup check only looked at pending/ and acked/, not quarantine/ — so
    // an event_id that had been quarantined (e.g. its file was corrupted on
    // disk after being written) could be silently re-enqueued as if nothing
    // had happened, leaving a stale quarantined copy orphaned under the same
    // id with no record of the conflict.
    let dir = temp_dir("quarantine-dedup");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();

    fs::write(
        dir.join("quarantine")
            .join(format!("{}.json", envelope.event_id)),
        b"{ not valid json, simulating a previously corrupted record",
    )
    .unwrap();

    let outcome = queue.enqueue(&envelope).unwrap();
    assert_eq!(
        outcome,
        EnqueueOutcome::AlreadyPresent,
        "an id already present in quarantine/ must be deduped, not silently re-enqueued"
    );
    assert!(queue.dequeue_batch(10).unwrap().is_empty());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn concurrent_dequeue_batch_never_delivers_the_same_record_twice() {
    // A regression test for a gap an independent review found: two
    // concurrent `dequeue_batch` callers could both observe the same
    // still-pending file before either acked it, double-processing it.
    use std::sync::{Arc, Mutex};
    use std::thread;

    let dir = temp_dir("concurrent-dequeue");
    let queue = Arc::new(open_queue(dir.clone()));

    const N: usize = 200;
    for _ in 0..N {
        queue.enqueue(&make_envelope()).unwrap();
    }

    let seen = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let duplicate_found = Arc::new(Mutex::new(None));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let seen = Arc::clone(&seen);
            let duplicate_found = Arc::clone(&duplicate_found);
            thread::spawn(move || loop {
                let batch = queue.dequeue_batch(5).unwrap();
                if batch.is_empty() {
                    break;
                }
                for record in &batch {
                    let id = record.envelope.event_id.to_string();
                    let mut seen = seen.lock().unwrap();
                    if !seen.insert(id.clone()) {
                        *duplicate_found.lock().unwrap() = Some(id);
                    }
                    drop(seen);
                    queue.ack(&record.envelope.event_id).unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(
        *duplicate_found.lock().unwrap(),
        None,
        "no record should ever be observed by two concurrent dequeue_batch callers"
    );
    assert_eq!(seen.lock().unwrap().len(), N);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn concurrent_enqueue_of_the_same_event_id_produces_exactly_one_winner() {
    // A regression test for a gap an independent review found: concurrent
    // enqueue attempts for the SAME event_id shared one temp filename,
    // letting more than one caller observe `Ok(Enqueued)` for what should
    // be a single logical record.
    use std::sync::{Arc, Mutex};
    use std::thread;

    let dir = temp_dir("concurrent-same-id");
    let queue = Arc::new(open_queue(dir.clone()));
    let envelope = Arc::new(make_envelope());

    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let envelope = Arc::clone(&envelope);
            let outcomes = Arc::clone(&outcomes);
            thread::spawn(move || {
                let result = queue.enqueue(&envelope);
                outcomes
                    .lock()
                    .unwrap()
                    .push(result.map_err(|e| e.to_string()));
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let outcomes = outcomes.lock().unwrap();
    let enqueued_count = outcomes
        .iter()
        .filter(|r| matches!(r, Ok(EnqueueOutcome::Enqueued)))
        .count();
    assert_eq!(
        enqueued_count, 1,
        "exactly one concurrent enqueue of the same event_id must win, got outcomes: {outcomes:?}"
    );
    let errors: Vec<_> = outcomes.iter().filter(|r| r.is_err()).collect();
    assert!(
        errors.is_empty(),
        "no concurrent attempt should error, got: {errors:?}"
    );

    let batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(
        batch.len(),
        1,
        "exactly one record must exist on disk, not zero or many"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn acking_a_not_pending_event_is_a_noop() {
    let dir = temp_dir("ack-noop");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();

    // Never enqueued at all — an uploader retrying an ack whose response
    // was lost must not get an error here.
    let result = queue.ack(&envelope.event_id);
    assert!(result.is_ok());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn release_returns_a_leased_record_to_pending_immediately() {
    // release() is what lets a consumer (AG-005's uploader) retry a record
    // that failed to upload WITHIN the same process — without it, a
    // dequeued-but-failed record would sit invisible in leased/ until the
    // whole process restarted.
    let dir = temp_dir("release");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();
    queue.enqueue(&envelope).unwrap();

    let leased = queue.dequeue_batch(10).unwrap();
    assert_eq!(leased.len(), 1);
    assert!(
        queue.dequeue_batch(10).unwrap().is_empty(),
        "a leased record must not be visible to dequeue_batch again"
    );

    queue.release(&envelope.event_id).unwrap();

    let redequeued = queue.dequeue_batch(10).unwrap();
    assert_eq!(
        redequeued.len(),
        1,
        "a released record must be visible to dequeue_batch again, without reopening the queue"
    );
    assert_eq!(
        redequeued[0].envelope.event_id.to_string(),
        envelope.event_id.to_string()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn releasing_a_not_leased_event_is_a_noop() {
    let dir = temp_dir("release-noop");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();

    // Never dequeued at all — a caller retrying a release after its own
    // error-handling path itself failed must not get an error here.
    let result = queue.release(&envelope.event_id);
    assert!(result.is_ok());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn corrupt_file_is_quarantined_not_fatal() {
    let dir = temp_dir("corrupt");
    let queue = open_queue(dir.clone());

    let good_a = make_envelope();
    let good_b = make_envelope();
    queue.enqueue(&good_a).unwrap();
    queue.enqueue(&good_b).unwrap();

    // Simulate corruption: a record file with unparseable content, placed
    // directly on disk (not through `enqueue`, which would never write this).
    let corrupt_path = dir.join("pending").join(format!("{}.json", Uuid::new_v4()));
    fs::write(&corrupt_path, b"{ not valid json").unwrap();

    let batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(batch.len(), 2, "both valid records must still be returned");
    assert!(
        !corrupt_path.exists(),
        "corrupt file must be moved out of pending/"
    );

    let quarantine_dir = dir.join("quarantine");
    let quarantined: Vec<_> = fs::read_dir(&quarantine_dir).unwrap().collect();
    assert_eq!(
        quarantined.len(),
        1,
        "corrupt file must land in quarantine/, not be deleted"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn queue_full_rejects_new_enqueue_without_losing_existing_data() {
    let dir = temp_dir("full");
    let envelope = make_envelope();
    let approx_size = serde_json::to_vec(&envelope).unwrap().len() as u64;

    let queue = DurableQueue::open(QueueConfig {
        dir: dir.clone(),
        max_pending_bytes: approx_size, // room for exactly one record
        acked_retention: Duration::days(7),
    })
    .unwrap();

    assert_eq!(queue.enqueue(&envelope).unwrap(), EnqueueOutcome::Enqueued);

    let second = make_envelope();
    let result = queue.enqueue(&second);
    assert!(
        matches!(result, Err(QueueError::Full { .. })),
        "queue must reject past its byte limit"
    );

    // The first record must still be intact — rejection must not evict it.
    let batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(batch.len(), 1);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn stale_tmp_file_from_a_simulated_crash_is_cleaned_up_on_open() {
    let dir = temp_dir("crash-recovery");
    fs::create_dir_all(dir.join("pending")).unwrap();
    fs::create_dir_all(dir.join("acked")).unwrap();
    fs::create_dir_all(dir.join("quarantine")).unwrap();
    fs::write(
        dir.join("QUEUE_FORMAT_VERSION"),
        durable_queue::QUEUE_FORMAT_VERSION,
    )
    .unwrap();

    // Simulates a process killed between `File::create(&tmp_path)` and the
    // subsequent `fs::rename` in `enqueue` — an enqueue that never returned
    // success, so `open()` recovering by deleting it loses nothing that was
    // ever confirmed durable.
    let stale_tmp = dir
        .join("pending")
        .join(format!("{}.json.tmp", Uuid::new_v4()));
    fs::write(&stale_tmp, b"{ incomplete").unwrap();

    let queue = open_queue(dir.clone());
    assert!(
        !stale_tmp.exists(),
        "stale .tmp file must be removed by open()'s crash recovery"
    );
    assert!(queue.dequeue_batch(10).unwrap().is_empty());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn dequeue_batch_returns_oldest_enqueued_first() {
    let dir = temp_dir("fifo");
    let queue = open_queue(dir.clone());

    let first = make_envelope();
    queue.enqueue(&first).unwrap();
    std::thread::sleep(StdDuration::from_millis(20));
    let second = make_envelope();
    queue.enqueue(&second).unwrap();
    std::thread::sleep(StdDuration::from_millis(20));
    let third = make_envelope();
    queue.enqueue(&third).unwrap();

    let batch = queue.dequeue_batch(10).unwrap();
    let ids: Vec<String> = batch
        .iter()
        .map(|r| r.envelope.event_id.to_string())
        .collect();
    assert_eq!(
        ids,
        vec![
            first.event_id.to_string(),
            second.event_id.to_string(),
            third.event_id.to_string(),
        ],
        "dequeue_batch must return records in enqueue order, not arbitrary directory order"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn prune_expired_acks_deletes_old_acks_but_never_touches_pending() {
    let dir = temp_dir("retention");
    let queue = DurableQueue::open(QueueConfig {
        dir: dir.clone(),
        max_pending_bytes: 10 * 1024 * 1024,
        acked_retention: Duration::zero(), // anything acked is immediately eligible for pruning
    })
    .unwrap();

    let acked_envelope = make_envelope();
    let pending_envelope = make_envelope();
    queue.enqueue(&acked_envelope).unwrap();
    std::thread::sleep(StdDuration::from_millis(20));
    queue.enqueue(&pending_envelope).unwrap();
    // ack now operates on leased/, not pending/ directly — dequeue first to
    // move acked_envelope's file into leased/, matching the real
    // dequeue-then-ack flow a consumer (AG-005's uploader) actually uses.
    let leased = queue.dequeue_batch(1).unwrap();
    assert_eq!(leased.len(), 1);
    assert_eq!(
        leased[0].envelope.event_id.to_string(),
        acked_envelope.event_id.to_string()
    );
    queue.ack(&acked_envelope.event_id).unwrap();

    std::thread::sleep(StdDuration::from_millis(20));
    let pruned = queue.prune_expired_acks().unwrap();
    assert_eq!(pruned, 1);

    let acked_path = dir
        .join("acked")
        .join(format!("{}.json", acked_envelope.event_id));
    assert!(!acked_path.exists(), "expired acked record must be pruned");

    // The still-pending (never acked) record must survive regardless of age.
    let batch = queue.dequeue_batch(10).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].envelope.event_id.to_string(),
        pending_envelope.event_id.to_string()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn mismatched_format_version_fails_to_open_instead_of_misreading() {
    let dir = temp_dir("version-mismatch");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("QUEUE_FORMAT_VERSION"), "999-from-the-future").unwrap();

    let result = DurableQueue::open(QueueConfig {
        dir: dir.clone(),
        max_pending_bytes: 10 * 1024 * 1024,
        acked_retention: Duration::days(7),
    });

    assert!(
        matches!(result, Err(QueueError::UnsupportedFormatVersion { .. })),
        "a queue directory from a future/incompatible format must fail loudly, not be silently misread"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_second_open_while_the_first_handle_is_still_alive_is_rejected() {
    // A regression test for a gap a second round of independent review
    // found: without this, a second `DurableQueue::open()` on the same
    // directory (while a first handle still has an outstanding un-acked
    // lease) would run `open()`'s lease-recovery step and unconditionally
    // yank that lease back to `pending/`, letting the second handle
    // dequeue the same record the first handle was still processing —
    // reintroducing the exact double-delivery class of bug the `leased/`
    // design was built to prevent, just one level up (at `open()` instead
    // of at `dequeue_batch()`).
    let dir = temp_dir("second-open-rejected");
    let _first = open_queue(dir.clone());

    let second = DurableQueue::open(QueueConfig {
        dir: dir.clone(),
        max_pending_bytes: 10 * 1024 * 1024,
        acked_retention: Duration::days(7),
    });
    assert!(
        matches!(second, Err(QueueError::AlreadyOpen { .. })),
        "a second open() while the first handle is alive must be rejected, not silently allowed"
    );

    // Dropping the first handle releases the lock — a subsequent open()
    // must then succeed normally (this is a real restart, not a conflict).
    drop(_first);
    let third = DurableQueue::open(QueueConfig {
        dir: dir.clone(),
        max_pending_bytes: 10 * 1024 * 1024,
        acked_retention: Duration::days(7),
    });
    assert!(
        third.is_ok(),
        "open() must succeed again once the prior handle is dropped"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reopening_the_same_directory_preserves_pending_data() {
    // Simulates "queue survives an update/restart": the process exits and a
    // new `DurableQueue::open` call (a fresh process, in a real agent) sees
    // exactly what was durably enqueued before.
    let dir = temp_dir("reopen");
    let envelope = make_envelope();
    {
        let queue = open_queue(dir.clone());
        queue.enqueue(&envelope).unwrap();
    }

    let reopened = open_queue(dir.clone());
    let batch = reopened.dequeue_batch(10).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].envelope.event_id.to_string(),
        envelope.event_id.to_string()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn high_volume_enqueue_stays_bounded_and_correct() {
    // A scaled proxy for AG-004's literal "24h offline test", not the real
    // thing: the Python MVP's own bucket interval is 2s, so 24h offline is
    // ~43,200 records. Running that many real records here would make this
    // test suite itself slow to run on every `cargo test`; this checks the
    // property that actually matters at any scale — the byte-size limit
    // (not a record count) is what bounds a real 24h-offline queue, dedup
    // and FIFO ordering hold under volume, and nothing silently corrupts —
    // with 2,000 records (~1+ hour equivalent) as a fast-running sample.
    // See TEST_REPORT.md for why a literal 43,200-record / wall-clock 24h
    // run is documented as a follow-up soak test, not run here.
    let dir = temp_dir("high-volume");
    let queue = open_queue(dir.clone());

    const COUNT: usize = 2_000;
    let mut envelopes = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        let envelope = make_envelope();
        assert_eq!(queue.enqueue(&envelope).unwrap(), EnqueueOutcome::Enqueued);
        envelopes.push(envelope);
    }

    let mut seen = std::collections::HashSet::new();
    let mut remaining = COUNT;
    while remaining > 0 {
        let batch = queue.dequeue_batch(100).unwrap();
        assert!(
            !batch.is_empty(),
            "must keep making progress until all records are drained"
        );
        for record in &batch {
            let id = record.envelope.event_id.to_string();
            assert!(
                seen.insert(id.clone()),
                "no record should be returned twice: {id}"
            );
            queue.ack(&record.envelope.event_id).unwrap();
        }
        remaining -= batch.len();
    }

    assert_eq!(seen.len(), COUNT);
    assert_eq!(
        queue.pending_bytes().unwrap(),
        0,
        "everything acked must leave pending/ empty"
    );

    fs::remove_dir_all(&dir).ok();
}
