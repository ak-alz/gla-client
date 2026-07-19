//! Resource-budget measurement for AG-LNX-002 (ADR 0013's per-process
//! budget). Prints its own PID so an external, real OS measurement
//! (`ps`/`/proc/<pid>/status`) can be taken against it — same discipline
//! as `windows-collector`'s `resource_soak.rs`.
//!
//! Named `linux_resource_soak`, not `resource_soak` — see
//! `linux_collector_demo.rs`'s doc comment for why (a real Cargo
//! output-filename collision with `windows-collector`'s own example of
//! the same name, found by independent review).
//!
//! Run with: cargo run -p linux-collector --example linux_resource_soak -- <seconds>

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("linux-collector's resource_soak only runs on Linux — nothing to do here.");
}

#[cfg(target_os = "linux")]
fn main() {
    use collector_core::SignalCollector;
    use linux_collector::{LinuxSignalCollector, NativeLoop};
    use std::env;
    use std::time::Duration;

    let seconds: u64 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    println!("PID={}", std::process::id());

    let mut collector = LinuxSignalCollector::new(120.0);
    let _ = collector.start(); // best-effort — this soak measures resource use regardless of which capabilities are available this session
    let native_loop_handle = NativeLoop::start().ok();

    for _ in 0..seconds {
        std::thread::sleep(Duration::from_secs(1));
        let _ = collector.poll();
    }

    collector.stop();
    if let Some((mut native_loop, _rx)) = native_loop_handle {
        native_loop.stop();
    }
    println!("done after {seconds}s");
}
