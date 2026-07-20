//! AG-PERF-001's `upload_burst` benchmark scenario needs a real backlog of
//! valid, drainable records pre-populating a queue directory before the
//! agent under test launches. An earlier version of `run.ps1` hand-wrote
//! approximated JSON envelope files directly — every one of them ended up
//! in `quarantine/` because the hand-guessed shape didn't match what
//! `Envelope::build_or_quarantine`/`DurableQueue::enqueue` actually produce
//! and expect. This tool uses the real API instead, so seeded records are
//! guaranteed to match exactly what a genuine collector would have written.
//!
//! Usage: `seed_queue <queue-dir> <count>`

use chrono::{Duration, Utc};
use durable_queue::{DurableQueue, QueueConfig};
use event_contract::{Consent, Envelope, NewEnvelope, Payload, Signals};
use std::path::PathBuf;
use uuid::Uuid;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir: PathBuf = args
        .next()
        .expect("usage: seed_queue <queue-dir> <count>")
        .into();
    let count: usize = args
        .next()
        .expect("usage: seed_queue <queue-dir> <count>")
        .parse()
        .expect("count must be a non-negative integer");

    let queue = DurableQueue::open(QueueConfig {
        dir,
        max_pending_bytes: u64::MAX,
        acked_retention: Duration::days(30),
    })
    .expect("failed to open queue directory");

    let device_id = event_contract::DeviceId::from_uuid(Uuid::new_v4());

    for i in 0..count {
        let period_end = Utc::now() - Duration::minutes(i as i64);
        let period_start = period_end - Duration::minutes(1);
        let payload = Payload {
            period_start,
            period_end,
            signals: Signals {
                active_seconds: 55.0,
                ..Default::default()
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
        };

        let envelope = Envelope::build_or_quarantine(NewEnvelope {
            device_id,
            agent_version: "0.1.0-rust-prototype".to_string(),
            payload,
        })
        .expect("seeded payload must pass validation -- this tool's own construction is a bug if it doesn't");

        queue
            .enqueue(&envelope)
            .expect("enqueue must succeed against a freshly-opened, empty-backlog queue directory");
    }

    println!("seeded {count} valid records");
}
