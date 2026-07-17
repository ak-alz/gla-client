//! Manual verification tool (not shipped): wires this crate's
//! single-instance guard, crash marker, and rotating log together with a
//! REAL tray from `ui-shell` (AG-007) in one process — the combined
//! resource profile this measures is what "Resource budget соблюден"
//! means for AG-008 (lifecycle's own overhead added on top of an already-
//! measured tray, not lifecycle in isolation, since lifecycle alone never
//! runs as a standalone product).
//!
//! Usage: `lifecycle_tray_demo <state_dir>` — acquires the single-instance
//! lock, checks/sets the crash marker, logs a startup line, then runs the
//! real tray until Quit, at which point it logs a clean-shutdown line and
//! clears the crash marker.
use lifecycle::{acquire, CrashMarker, RotatingLog};
use std::path::PathBuf;
use std::sync::Arc;
use ui_shell::{run_tray, AgentController, AgentStatus};

struct DemoController;

impl AgentController for DemoController {
    fn status(&self) -> AgentStatus {
        AgentStatus {
            paired: true,
            is_paused: false,
            last_sync: Some(chrono::Utc::now()),
            pending_count: 0,
            agent_version: "0.1.0-rust-ag008-demo".to_string(),
        }
    }
    fn toggle_active(&self) {}
    fn dashboard_url(&self) -> String {
        "http://localhost:5173".to_string()
    }
    fn diagnostics_url(&self) -> String {
        "http://localhost:5173/history".to_string()
    }
    fn help_url(&self) -> String {
        "https://github.com/ak-alz/pts-agent".to_string()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let state_dir = PathBuf::from(args.get(1).expect("usage: lifecycle_tray_demo <state_dir>"));
    std::fs::create_dir_all(&state_dir).unwrap();

    let _instance_guard = match acquire(&state_dir.join("instance.lock")) {
        Ok(guard) => guard,
        Err(err) => {
            println!("ALREADY_RUNNING: {err}");
            return;
        }
    };

    let log = RotatingLog::new(state_dir.join("logs"), "lifecycle.log", 1_000_000, 5).unwrap();
    let crash_marker = CrashMarker::new(state_dir.join("running.marker"));

    if crash_marker.previous_run_crashed() {
        log.append("previous run did not exit cleanly (crash detected)")
            .unwrap();
        println!("PREVIOUS_RUN_CRASHED");
    } else {
        println!("PREVIOUS_RUN_CLEAN_OR_FIRST_RUN");
    }
    crash_marker.mark_running().unwrap();
    log.append("agent started").unwrap();

    println!("READY pid={}", std::process::id());
    run_tray(Arc::new(DemoController)).expect("tray must start");

    // Only reached via the tray's own Quit action (a clean shutdown path).
    log.append("agent quit cleanly").unwrap();
    crash_marker.mark_clean_exit().unwrap();
    println!("CLEAN_EXIT");
}
