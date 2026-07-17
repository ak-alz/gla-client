//! Resource-budget measurement for AG-WIN-001 (ADR 0013's per-process
//! budget). Starts the real collector plus the real `NativeLoop`, polls
//! at a realistic ~1s cadence for a sustained period, and prints its own
//! PID so an external, real OS measurement (`Get-Process`) can be taken
//! against it — the same "measure the actual running process" discipline
//! already used for AG-008's resource-budget check, not an estimate.
//!
//! Run with: cargo run -p windows-collector --example resource_soak -- <seconds>

use std::collections::HashSet;
use std::env;
use std::time::Duration;
use windows_collector::{NativeLoop, SignalCollector, WindowsSignalCollector};

fn main() {
    let seconds: u64 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    println!("PID={}", std::process::id());

    let mut collector = WindowsSignalCollector::new(120.0, HashSet::<String>::new(), Vec::new());
    collector.start().expect("collector start must succeed");
    let (mut native_loop, _rx) = NativeLoop::start().expect("native loop start must succeed");

    let ticks = seconds;
    for _ in 0..ticks {
        std::thread::sleep(Duration::from_secs(1));
        let _ = collector.poll();
    }

    collector.stop();
    native_loop.stop();
    println!("done after {seconds}s");
}
