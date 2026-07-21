//! Manual verification tool (not shipped, not part of the crate's public
//! API): runs a real tray icon on the main thread while a background
//! thread independently advances a simulated "last sync"/"pending count"
//! state on its own timer — concretely demonstrating "UI не нужен для
//! работы collector" / "Collector продолжает работать без открытого UI":
//! the background thread's progress does not depend on the tray being
//! open, clicked, or even rendered successfully, and closing the tray
//! (Quit) only exits the tray's own event loop, never touches the
//! background thread.
//!
//! Also used for real resource-profile verification (process inspected via
//! `Get-Process`/`Get-CimInstance` while this runs — see TEST_REPORT.md)
//! — this is the actual, non-benchmark tray implementation this task
//! built, run for real, not AG-002's throwaway prototype.
//!
//! Writes one line per background tick to `tray_demo_log.txt` in the
//! current directory so the "still advancing regardless of the tray"
//! claim can be checked from outside the process (e.g. `tail -f` in
//! another terminal) while the tray runs.
use chrono::Utc;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ui_shell::{run_tray, AgentController, AgentStatus};

struct DemoController {
    paused: AtomicBool,
    pending_count: AtomicUsize,
    last_sync: Mutex<Option<chrono::DateTime<Utc>>>,
}

impl AgentController for DemoController {
    fn status(&self) -> AgentStatus {
        AgentStatus {
            paired: true,
            is_paused: self.paused.load(Ordering::Relaxed),
            last_sync: *self.last_sync.lock().unwrap(),
            pending_count: self.pending_count.load(Ordering::Relaxed),
            agent_version: "0.1.0-rust-ag007-demo".to_string(),
        }
    }

    fn toggle_active(&self) {
        let was_paused = self.paused.fetch_xor(true, Ordering::Relaxed);
        println!(
            "toggled: now {}",
            if was_paused { "active" } else { "paused" }
        );
    }

    fn dashboard_url(&self) -> String {
        "http://localhost:5173".to_string()
    }

    fn diagnostics_url(&self) -> String {
        "http://localhost:5173/history".to_string()
    }

    fn help_url(&self) -> String {
        "https://github.com/ak-alz/gla-client".to_string()
    }

    fn pair_device(&self) {
        println!("pair_device: demo controller is always 'paired', nothing to do");
    }
}

fn main() {
    let controller = Arc::new(DemoController {
        paused: AtomicBool::new(false),
        pending_count: AtomicUsize::new(0),
        last_sync: Mutex::new(None),
    });

    // The "collector" stand-in: a completely independent background
    // thread. Whether the tray on the main thread is open, closed, or
    // never even successfully rendered has no bearing on this loop.
    let background_controller = Arc::clone(&controller);
    std::thread::spawn(move || {
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("tray_demo_log.txt")
            .expect("open demo log");
        let mut tick: u64 = 0;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            tick += 1;
            if !background_controller.paused.load(Ordering::Relaxed) {
                background_controller
                    .pending_count
                    .fetch_add(1, Ordering::Relaxed);
                *background_controller.last_sync.lock().unwrap() = Some(Utc::now());
            }
            let _ = writeln!(
                log,
                "tick={tick} paused={} pending={}",
                background_controller.paused.load(Ordering::Relaxed),
                background_controller.pending_count.load(Ordering::Relaxed)
            );
        }
    });

    println!("Tray demo running. Close via the tray's Quit item (background thread keeps its own console/log output regardless).");
    run_tray(controller).expect("tray must start");
}
