//! Contract tests for the uploader's retry/backoff state machine, using a
//! scripted `MockTransport` — deterministic fault injection (429/5xx/401/
//! network-error sequences) that a real, shared dev backend cannot be made
//! to produce on demand without disrupting it. The happy path (a real
//! upload succeeding against the real backend) is verified separately,
//! against a live server — see TEST_REPORT.md.

use chrono::Duration as ChronoDuration;
use durable_queue::{DurableQueue, QueueConfig};
use event_contract::{Consent, DeviceId, Envelope, NewEnvelope, Payload, Signals};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use uploader::{
    BackoffConfig, BackoffReason, BackoffState, CycleOutcome, TransportOutcome, UploadTransport,
    Uploader, UploaderConfig,
};
use uuid::Uuid;

struct MockTransport {
    scripted: Mutex<Vec<TransportOutcome>>,
    calls: Mutex<Vec<Vec<u8>>>,
}

impl MockTransport {
    fn new(scripted: Vec<TransportOutcome>) -> Self {
        MockTransport {
            scripted: Mutex::new(scripted),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl UploadTransport for MockTransport {
    fn post_record(&self, body: &[u8]) -> TransportOutcome {
        self.calls.lock().unwrap().push(body.to_vec());
        let mut scripted = self.scripted.lock().unwrap();
        if scripted.is_empty() {
            panic!("MockTransport called more times than scripted");
        }
        scripted.remove(0)
    }
}

fn temp_queue_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("uploader-test-{label}-{}", Uuid::new_v4()))
}

fn open_queue(dir: PathBuf) -> DurableQueue {
    DurableQueue::open(QueueConfig {
        dir,
        max_pending_bytes: 10 * 1024 * 1024,
        acked_retention: ChronoDuration::days(7),
    })
    .expect("queue must open")
}

fn make_envelope() -> Envelope {
    let period_end = "2026-07-17T12:00:00Z".parse().unwrap();
    let period_start = period_end - ChronoDuration::seconds(120);
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

fn test_config() -> UploaderConfig {
    UploaderConfig {
        batch_size: 10,
        backoff: BackoffConfig {
            base: Duration::from_millis(1), // fast in tests; growth/cap behavior is covered separately in backoff.rs's own unit tests
            max: Duration::from_secs(60),
            jitter_fraction: 0.0,
        },
    }
}

#[test]
fn idle_when_queue_is_empty() {
    let dir = temp_queue_dir("idle");
    let queue = open_queue(dir.clone());
    let transport = MockTransport::new(vec![]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    assert_eq!(uploader.run_once(&queue, &mut state), CycleOutcome::Idle);
    assert_eq!(transport.call_count(), 0);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn successful_upload_acks_and_reports_progress() {
    let dir = temp_queue_dir("success");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();
    queue.enqueue(&envelope).unwrap();

    let transport = MockTransport::new(vec![TransportOutcome::Success { status: 201 }]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    let outcome = uploader.run_once(&queue, &mut state);
    assert_eq!(
        outcome,
        CycleOutcome::Progress {
            uploaded: 1,
            client_errors: 0
        }
    );
    assert_eq!(state.consecutive_failures(), 0);

    // The record must actually be acked, not just reported as uploaded.
    assert!(queue.dequeue_batch(10).unwrap().is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn server_error_stops_the_batch_and_leaves_the_record_pending() {
    let dir = temp_queue_dir("server-error");
    let queue = open_queue(dir.clone());
    queue.enqueue(&make_envelope()).unwrap();
    queue.enqueue(&make_envelope()).unwrap();

    let transport = MockTransport::new(vec![TransportOutcome::ServerError { status: 503 }]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    let outcome = uploader.run_once(&queue, &mut state);
    match outcome {
        CycleOutcome::Backoff {
            reason: BackoffReason::ServerError,
            after,
        } => {
            assert!(after.as_millis() > 0);
        }
        other => panic!("expected Backoff/ServerError, got {other:?}"),
    }
    assert_eq!(
        transport.call_count(),
        1,
        "must not attempt the second record after the first fails"
    );

    // Both records must still be pending — a server failure must not lose
    // (or ack) anything.
    assert_eq!(queue.dequeue_batch(10).unwrap().len(), 2);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn network_error_is_treated_like_offline_and_backs_off() {
    let dir = temp_queue_dir("network-error");
    let queue = open_queue(dir.clone());
    queue.enqueue(&make_envelope()).unwrap();

    let transport = MockTransport::new(vec![TransportOutcome::NetworkError]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    let outcome = uploader.run_once(&queue, &mut state);
    assert!(matches!(
        outcome,
        CycleOutcome::Backoff {
            reason: BackoffReason::NetworkError,
            ..
        }
    ));
    assert_eq!(
        queue.dequeue_batch(10).unwrap().len(),
        1,
        "nothing lost while offline"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rate_limited_honors_server_retry_after_header_over_computed_backoff() {
    let dir = temp_queue_dir("rate-limited");
    let queue = open_queue(dir.clone());
    queue.enqueue(&make_envelope()).unwrap();

    let transport = MockTransport::new(vec![TransportOutcome::RateLimited {
        retry_after: Some(Duration::from_secs(42)),
    }]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    let outcome = uploader.run_once(&queue, &mut state);
    match outcome {
        CycleOutcome::Backoff {
            reason: BackoffReason::RateLimited,
            after,
        } => {
            assert_eq!(
                after,
                Duration::from_secs(42),
                "server's Retry-After must win over our own computed backoff"
            );
        }
        other => panic!("expected Backoff/RateLimited, got {other:?}"),
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rate_limited_without_retry_after_falls_back_to_computed_backoff() {
    let dir = temp_queue_dir("rate-limited-no-header");
    let queue = open_queue(dir.clone());
    queue.enqueue(&make_envelope()).unwrap();

    let transport = MockTransport::new(vec![TransportOutcome::RateLimited { retry_after: None }]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    match uploader.run_once(&queue, &mut state) {
        CycleOutcome::Backoff {
            reason: BackoffReason::RateLimited,
            after,
        } => {
            assert!(after.as_millis() > 0);
        }
        other => panic!("expected Backoff/RateLimited, got {other:?}"),
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unauthorized_short_circuits_without_backoff_and_without_acking() {
    let dir = temp_queue_dir("unauthorized");
    let queue = open_queue(dir.clone());
    queue.enqueue(&make_envelope()).unwrap();

    let transport = MockTransport::new(vec![TransportOutcome::Unauthorized]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    assert_eq!(
        uploader.run_once(&queue, &mut state),
        CycleOutcome::Unauthorized
    );
    assert_eq!(
        queue.dequeue_batch(10).unwrap().len(),
        1,
        "record must not be acked on a rejected token"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn client_error_does_not_stop_the_batch_and_is_not_acked() {
    let dir = temp_queue_dir("client-error");
    let queue = open_queue(dir.clone());
    let bad = make_envelope();
    let good = make_envelope();
    queue.enqueue(&bad).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    queue.enqueue(&good).unwrap();

    let transport = MockTransport::new(vec![
        TransportOutcome::ClientError { status: 422 },
        TransportOutcome::Success { status: 201 },
    ]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    let outcome = uploader.run_once(&queue, &mut state);
    assert_eq!(
        outcome,
        CycleOutcome::Progress {
            uploaded: 1,
            client_errors: 1
        }
    );
    assert_eq!(
        transport.call_count(),
        2,
        "a per-record client error must not stop the rest of the batch"
    );

    // The rejected record must still be pending (inspectable, not lost);
    // the successful one must be gone.
    let remaining = queue.dequeue_batch(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].envelope.event_id.to_string(),
        bad.event_id.to_string()
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn backoff_state_grows_across_repeated_failures_and_resets_on_success() {
    let dir = temp_queue_dir("backoff-growth");
    let queue = open_queue(dir.clone());
    for _ in 0..3 {
        queue.enqueue(&make_envelope()).unwrap();
    }

    let transport = MockTransport::new(vec![
        TransportOutcome::ServerError { status: 503 },
        TransportOutcome::ServerError { status: 503 },
        TransportOutcome::Success { status: 201 },
    ]);
    // batch_size: 1 so each `run_once` call attempts exactly one record —
    // otherwise a success wouldn't stop the batch (only failures do, see
    // `run_once`'s doc comment) and this call would keep going into R2/R3
    // with no more scripted responses left.
    let config = UploaderConfig {
        batch_size: 1,
        backoff: test_config().backoff,
    };
    let uploader = Uploader::new(&transport, config);
    let mut state = BackoffState::new();

    let first = match uploader.run_once(&queue, &mut state) {
        CycleOutcome::Backoff { after, .. } => after,
        other => panic!("expected Backoff, got {other:?}"),
    };
    assert_eq!(state.consecutive_failures(), 1);

    let second = match uploader.run_once(&queue, &mut state) {
        CycleOutcome::Backoff { after, .. } => after,
        other => panic!("expected Backoff, got {other:?}"),
    };
    assert_eq!(state.consecutive_failures(), 2);
    assert!(
        second >= first,
        "backoff must not shrink between consecutive failures"
    );

    let third = uploader.run_once(&queue, &mut state);
    assert_eq!(
        third,
        CycleOutcome::Progress {
            uploaded: 1,
            client_errors: 0
        }
    );
    assert_eq!(
        state.consecutive_failures(),
        0,
        "a success must reset the failure count"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_transient_failure_does_not_lose_the_record_and_it_uploads_on_the_next_attempt() {
    // Simulates the real offline->online transition end to end at the
    // uploader's own level: first attempt fails (network down), second
    // attempt (network back) succeeds — same record, not re-created.
    let dir = temp_queue_dir("offline-then-online");
    let queue = open_queue(dir.clone());
    let envelope = make_envelope();
    queue.enqueue(&envelope).unwrap();

    let transport = MockTransport::new(vec![
        TransportOutcome::NetworkError,
        TransportOutcome::Success { status: 201 },
    ]);
    let uploader = Uploader::new(&transport, test_config());
    let mut state = BackoffState::new();

    assert!(matches!(
        uploader.run_once(&queue, &mut state),
        CycleOutcome::Backoff { .. }
    ));
    assert_eq!(
        uploader.run_once(&queue, &mut state),
        CycleOutcome::Progress {
            uploaded: 1,
            client_errors: 0
        }
    );
    assert!(queue.dequeue_batch(10).unwrap().is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn no_agent_token_or_record_content_leaks_through_transport_outcome() {
    // TransportOutcome's variants structurally carry only status codes and
    // durations — never a body or token — so nothing calling `run_once`
    // could accidentally log payload/token content even if it tried to log
    // the outcome directly. This is a structural guarantee, verified by
    // exhaustively matching every variant's fields.
    fn assert_no_sensitive_payload(outcome: &TransportOutcome) {
        match outcome {
            TransportOutcome::Success { status } => {
                let _: &u16 = status;
            }
            TransportOutcome::RateLimited { retry_after } => {
                let _: &Option<Duration> = retry_after;
            }
            TransportOutcome::ServerError { status } => {
                let _: &u16 = status;
            }
            TransportOutcome::Unauthorized => {}
            TransportOutcome::ClientError { status } => {
                let _: &u16 = status;
            }
            TransportOutcome::NetworkError => {}
        }
    }
    assert_no_sensitive_payload(&TransportOutcome::Success { status: 201 });
}
