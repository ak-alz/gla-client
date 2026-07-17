//! Throwaway verification tool (not shipped, not part of the crate's public
//! API): builds one realistic envelope and prints its `to_legacy_wire()` JSON
//! to stdout, so it can be piped straight into a real backend's ingest
//! endpoint via curl — proving AG-003's "compatible with backend" criterion
//! against a live server, not just a hand-maintained mirror struct.
use event_contract::{
    Consent, DeviceId, Envelope, InputActivityEvents, NewEnvelope, Payload, Signals,
};
use std::collections::BTreeMap;
use uuid::Uuid;

fn main() {
    let period_end = chrono::Utc::now();
    let period_start = period_end - chrono::Duration::seconds(120);

    let mut active_app_category_seconds = BTreeMap::new();
    active_app_category_seconds.insert("deep_work".to_string(), 90.0);
    active_app_category_seconds.insert("communication".to_string(), 30.0);

    let payload = Payload {
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
    };

    let envelope = Envelope::try_new(NewEnvelope {
        device_id: DeviceId::from_uuid(Uuid::new_v4()),
        agent_version: "0.1.0-rust-ag003-verification".to_string(),
        payload,
    })
    .expect("hand-built payload must be valid");

    println!(
        "{}",
        serde_json::to_string(&envelope.to_legacy_wire()).unwrap()
    );
}
