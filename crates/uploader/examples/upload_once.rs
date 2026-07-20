//! Throwaway verification tool (not shipped, not part of the crate's public
//! API): builds one realistic envelope, enqueues it into a fresh
//! `DurableQueue` at the given directory, and runs the real `Uploader` +
//! `UreqTransport` pipeline against a real backend for exactly one cycle —
//! proving AG-005's "offline/online transition works" happy path and
//! "upload resumes after reboot" (via the durable queue underneath) against
//! a live server, not just a hand-maintained mock.
//!
//! Usage: `upload_once <queue_dir> <backend_url> <agent_token>`
//! `backend_url` is the base origin (e.g. `http://localhost:8000`) --
//! `UreqTransport::new` appends `/v1/ingest/productivity-record` itself
//! (AG-REL-003 fix; this tool's own arg used to need the full path before
//! that fix existed).
use event_contract::{
    Consent, DeviceId, Envelope, InputActivityEvents, NewEnvelope, Payload, Signals,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use uploader::{BackoffConfig, BackoffState, Uploader, UploaderConfig, UreqTransport};
use uuid::Uuid;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let queue_dir = PathBuf::from(
        args.get(1)
            .expect("usage: upload_once <queue_dir> <endpoint> <agent_token>"),
    );
    let endpoint = args.get(2).expect("missing endpoint").clone();
    let agent_token = args.get(3).expect("missing agent_token").clone();

    let queue = durable_queue::DurableQueue::open(durable_queue::QueueConfig {
        dir: queue_dir,
        max_pending_bytes: 10 * 1024 * 1024,
        acked_retention: chrono::Duration::days(7),
    })
    .expect("queue must open");

    let period_end = chrono::Utc::now();
    let period_start = period_end - chrono::Duration::seconds(120);
    let mut active_app_category_seconds = BTreeMap::new();
    active_app_category_seconds.insert("deep_work".to_string(), 90.0);
    active_app_category_seconds.insert("communication".to_string(), 30.0);

    let envelope = Envelope::try_new(NewEnvelope {
        device_id: DeviceId::from_uuid(Uuid::new_v4()),
        agent_version: "0.1.0-rust-ag005-verification".to_string(),
        payload: Payload {
            period_start,
            period_end,
            signals: Signals {
                active_app_category_seconds: Some(active_app_category_seconds),
                input_activity_events: Some(InputActivityEvents {
                    keyboard: 240,
                    mouse_move: 80,
                    mouse_click: 12,
                }),
                idle_seconds: Some(0.0),
                active_seconds: 120.0,
                activity_segments: None,
                unexplained_gaps: None,
                git_commits_count: None,
                app_seconds: None,
                other_app_seconds: None,
            },
            consent: Consent {
                active_app_category: true,
                input_activity_counts: true,
                idle_tracking: true,
                activity_segments: false,
                unexplained_gaps: false,
                git_activity: false,
                app_detail: false,
            },
            signature: None,
        },
    })
    .expect("hand-built payload must be valid");

    println!("enqueuing event_id={}", envelope.event_id);
    let outcome = queue.enqueue(&envelope).expect("enqueue must succeed");
    println!("enqueue outcome: {outcome:?}");

    let transport = UreqTransport::new(endpoint, agent_token, Duration::from_secs(10));
    let uploader = Uploader::new(
        &transport,
        UploaderConfig {
            batch_size: 10,
            backoff: BackoffConfig::default(),
        },
    );
    let mut state = BackoffState::new();

    let cycle = uploader.run_once(&queue, &mut state);
    println!("cycle outcome: {cycle:?}");

    let remaining = queue.dequeue_batch(10).expect("dequeue_batch must succeed");
    println!("remaining pending after cycle: {}", remaining.len());
}
