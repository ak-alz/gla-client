//! Real, live verification for AG-WIN-001 — no mocking. Run with:
//!
//!   cargo run -p windows-collector --example collector_demo
//!
//! What this proves, all via real Win32 calls (not simulated in-process
//! state): the real low-level keyboard/mouse hooks intercept real input
//! (synthesized here via `SendInput`, which the OS cannot distinguish from
//! physical hardware input — the same technique UI-automation tools use),
//! the real idle timer via `GetLastInputInfo` reflects that same input,
//! the real foreground window/process lookup resolves to whatever this
//! terminal session's window actually is, and the real `NativeLoop`
//! window/session-notification registration starts and stops cleanly.
//!
//! `SendInput` is used instead of asking a human to type during a CI-less
//! manual run, and deliberately only ever sends a bare Shift key
//! press/release (types no character into whatever has focus) and a
//! *relative* 1px mouse move-and-back (never a click — a real injected
//! click could land on and activate whatever is under the cursor on this
//! live, interactive desktop; see `hooks.rs`'s doc comment on why mouse
//! click COUNTING is instead verified by code inspection, sharing the
//! exact same dispatch mechanism this demo does exercise for move).

use std::collections::HashSet;
use std::time::Duration;
use windows_collector::{NativeLoop, RawSignalSnapshot, SignalCollector, WindowsSignalCollector};

fn print_snapshot(label: &str, snap: &RawSignalSnapshot) {
    println!(
        "[{label}] process={:?} keyboard={} mouse_move={} mouse_click={} idle={:.3}s is_idle={} category_override={:?}",
        snap.active_process_name,
        snap.keyboard_events,
        snap.mouse_move_events,
        snap.mouse_click_events,
        snap.idle_seconds,
        snap.is_idle,
        snap.category_override
    );
}

#[cfg(windows)]
fn send_synthetic_shift_key() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_SHIFT,
    };

    fn keyboard_input(flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_SHIFT,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let inputs = [keyboard_input(0), keyboard_input(KEYEVENTF_KEYUP)];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    println!("(injected {sent}/2 synthetic Shift key events via SendInput)");
}

#[cfg(windows)]
fn send_synthetic_mouse_move_and_back() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
    };

    fn mouse_input(dx: i32, dy: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let inputs = [mouse_input(1, 1), mouse_input(-1, -1)];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    println!("(injected {sent}/2 synthetic 1px-and-back mouse moves via SendInput)");
}

fn main() {
    println!("=== windows-collector real live verification ===");

    let mut collector = WindowsSignalCollector::new(3.0, HashSet::<String>::new(), Vec::new());
    collector
        .start()
        .expect("real hook installation must succeed");

    std::thread::sleep(Duration::from_millis(300));
    print_snapshot("baseline", &collector.poll());

    #[cfg(windows)]
    {
        send_synthetic_shift_key();
        send_synthetic_mouse_move_and_back();
    }
    std::thread::sleep(Duration::from_millis(300));
    print_snapshot(
        "after synthetic input (expect keyboard=1, mouse_move>=1)",
        &collector.poll(),
    );

    println!(
        "(sleeping 4s with no further input — expect idle_seconds to grow past the 3s threshold)"
    );
    std::thread::sleep(Duration::from_secs(4));
    print_snapshot("after 4s idle (expect is_idle=true)", &collector.poll());

    collector.stop();
    println!("collector stopped cleanly.");

    println!("--- native_loop (session/power notification registration) ---");
    match NativeLoop::start() {
        Ok((mut native_loop, rx)) => {
            println!(
                "NativeLoop started (real window + WTSRegisterSessionNotification succeeded)."
            );
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(notification) => println!("received real notification: {notification:?}"),
                Err(_) => println!(
                    "no notification in 500ms — expected: this run never actually suspends/locks \
                     the live session (see native_loop.rs's own #[cfg(test)] round-trip tests for \
                     real, synthetic WM_WTSSESSION_CHANGE/WM_POWERBROADCAST verification instead)."
                ),
            }
            native_loop.stop();
            println!("NativeLoop stopped cleanly.");
        }
        Err(err) => println!("NativeLoop::start() failed: {err}"),
    }
}
