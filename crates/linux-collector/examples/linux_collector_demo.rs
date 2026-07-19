//! Real, live verification for AG-LNX-002 — no mocking. Run with:
//!
//!   cargo run -p linux-collector --example linux_collector_demo
//!
//! Exercises the real X11 backend against whatever real X11/XWayland
//! session is running (this crate's own dev environment: WSLg, running
//! Weston + XWayland) — real EWMH property reads, real XScreenSaver
//! idle query. Also starts the real `NativeLoop` (real D-Bus connection,
//! real `AddMatch` calls against the real system bus/logind).
//!
//! What this demo CANNOT show for real in this dev environment (see
//! each module's own doc comment for why, and TEST_REPORT.md for the
//! full accounting): input-event counts (no `/dev/input/event*` nodes
//! exist in this WSL sandbox — `poll()` will honestly report zero counts
//! and `is_idle=true` immediately, which is the CORRECT behavior for "no
//! input capability available," not a bug) and Hyprland (WSLg runs
//! Weston, not Hyprland).
//!
//! Named `linux_collector_demo`, not `collector_demo` — an independent
//! review found `cargo build --workspace --examples` genuinely warns
//! about an output-filename collision with `windows-collector`'s own
//! `examples/collector_demo.rs` (both would produce
//! `target/debug/examples/collector_demo.exe`); Cargo calls this
//! "may become a hard error in the future." Renamed here rather than in
//! the already-shipped `windows-collector`.

#[cfg(target_os = "linux")]
use collector_core::SignalCollector;
#[cfg(target_os = "linux")]
use linux_collector::{LinuxSignalCollector, NativeLoop};
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("linux-collector's collector_demo only runs on Linux — nothing to do here.");
}

#[cfg(target_os = "linux")]
fn main() {
    println!("=== linux-collector real live verification ===");

    let mut collector = LinuxSignalCollector::new(3.0);
    match collector.start() {
        Ok(()) => println!("collector started"),
        Err(err) => {
            println!("collector start() failed: {err} (expected if no X11/Wayland session)");
            return;
        }
    }

    if let Some(reason) = collector.unsupported_reason() {
        println!("active-window backend unsupported this session: {reason:?}");
    }

    for i in 0..3 {
        std::thread::sleep(Duration::from_millis(500));
        let snap = collector.poll();
        println!(
            "[{i}] process={:?} keyboard={} mouse_move={} mouse_click={} idle={:.3}s is_idle={}",
            snap.active_process_name,
            snap.keyboard_events,
            snap.mouse_move_events,
            snap.mouse_click_events,
            snap.idle_seconds,
            snap.is_idle
        );
    }

    collector.stop();
    println!("collector stopped cleanly.");

    println!("--- native_loop (session/power notification registration) ---");
    match NativeLoop::start() {
        Ok((mut native_loop, rx)) => {
            println!("NativeLoop started (real D-Bus system-bus connection + AddMatch succeeded).");
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(event) => println!("received real notification: {event:?}"),
                Err(_) => println!(
                    "no notification in 500ms — expected: this run never actually locks/suspends \
                     the session (see native_loop.rs's own tests for real, loginctl-triggered \
                     Lock/Unlock verification instead)."
                ),
            }
            native_loop.stop();
            println!("NativeLoop stopped cleanly.");
        }
        Err(err) => println!("NativeLoop::start() failed: {err}"),
    }
}
