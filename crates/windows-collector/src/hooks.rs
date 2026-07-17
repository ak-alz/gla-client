//! Real Win32 low-level keyboard/mouse hooks — the Rust equivalent of the
//! Python source's `pynput.keyboard.Listener`/`pynput.mouse.Listener`.
//! Counts key-down and mouse-move/click events only; never inspects which
//! key, which button beyond down-vs-move, or any coordinate, matching the
//! Python source's `_on_key_press`/`_on_mouse_move`/`_on_mouse_click`
//! handlers exactly (increment a counter, discard the rest of the
//! argument).
//!
//! `WH_KEYBOARD_LL`/`WH_MOUSE_LL` hook procedures are plain
//! `extern "system" fn` pointers with no user-data slot (unlike
//! `CreateWindowExW`'s `lpCreateParams` used in `native_loop.rs`), so the
//! counters they increment must be a process-wide `static` — see
//! `input_counters.rs` for why the counting logic itself is still fully
//! unit-testable despite that. Installing/pumping/removing the hooks
//! themselves needs a real Windows message loop on the installing thread
//! (an OS requirement, not a design choice) — that real, live behavior is
//! verified via `examples/collector_demo.rs` (type/click while it runs,
//! watch the counts move) rather than an automated test, for the same
//! reason `crash_restart.rs` in the `lifecycle` crate could only verify
//! the registration call, not full end-to-end OS behavior: a `cargo test`
//! run must not install a real global input hook that could intercept (or,
//! if buggy, swallow) the developer's actual keystrokes/clicks during a
//! parallel test run on their live, interactive machine.

use crate::input_counters::InputCounters;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use thiserror::Error;

static COUNTERS: InputCounters = InputCounters::new();

/// Counts since the last call, reset to zero — the public entry point
/// `collector.rs` polls once per tick.
pub fn take_and_reset_counts() -> (i64, i64, i64) {
    COUNTERS.take_and_reset()
}

#[derive(Debug, Error)]
pub enum InputHooksError {
    #[error("failed to install the low-level keyboard hook")]
    KeyboardHookFailed,
    #[error("failed to install the low-level mouse hook")]
    MouseHookFailed,
    #[error("the hook-installer thread ended before reporting readiness")]
    ThreadDiedBeforeReady,
}

#[cfg(windows)]
mod imp {
    use super::{InputHooksError, COUNTERS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HHOOK, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN,
        WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_SYSKEYDOWN,
    };

    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        wparam: windows_sys::Win32::Foundation::WPARAM,
        lparam: windows_sys::Win32::Foundation::LPARAM,
    ) -> windows_sys::Win32::Foundation::LRESULT {
        if code >= 0 {
            let message = wparam as u32;
            if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
                COUNTERS.record_keyboard();
            }
        }
        // MUST always forward the event — swallowing it here would break
        // keyboard input system-wide for the whole interactive session.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    unsafe extern "system" fn mouse_hook_proc(
        code: i32,
        wparam: windows_sys::Win32::Foundation::WPARAM,
        lparam: windows_sys::Win32::Foundation::LPARAM,
    ) -> windows_sys::Win32::Foundation::LRESULT {
        if code >= 0 {
            match wparam as u32 {
                WM_MOUSEMOVE => COUNTERS.record_mouse_move(),
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => COUNTERS.record_mouse_click(),
                _ => {}
            }
        }
        // Same reasoning as keyboard_hook_proc: must always forward.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    pub(super) fn run_message_loop(ready: std::sync::mpsc::Sender<Result<u32, InputHooksError>>) {
        let thread_id = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };

        let keyboard_hook: HHOOK = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook_proc),
                std::ptr::null_mut(),
                0,
            )
        };
        if keyboard_hook.is_null() {
            let _ = ready.send(Err(InputHooksError::KeyboardHookFailed));
            return;
        }

        let mouse_hook: HHOOK = unsafe {
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), std::ptr::null_mut(), 0)
        };
        if mouse_hook.is_null() {
            unsafe {
                UnhookWindowsHookEx(keyboard_hook);
            }
            let _ = ready.send(Err(InputHooksError::MouseHookFailed));
            return;
        }

        if ready.send(Ok(thread_id)).is_err() {
            // Caller already gave up (e.g. start() timed out) — clean up
            // and exit rather than pumping messages nobody will stop.
            unsafe {
                UnhookWindowsHookEx(keyboard_hook);
                UnhookWindowsHookEx(mouse_hook);
            }
            return;
        }

        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            // WM_QUIT (posted by `stop()`) makes GetMessageW return 0.
            let result = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        unsafe {
            UnhookWindowsHookEx(keyboard_hook);
            UnhookWindowsHookEx(mouse_hook);
        }
    }

    pub(super) fn post_quit(thread_id: u32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        unsafe {
            PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
        }
    }
}

/// Owns the background thread that installs, pumps, and removes the
/// low-level keyboard/mouse hooks. Dropping it (or calling `stop()`)
/// cleanly unhooks and joins the thread — mirrors `stop()` in the Python
/// source, which stops both `pynput` listeners.
pub struct InputHooks {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl InputHooks {
    #[cfg(windows)]
    pub fn start() -> Result<Self, InputHooksError> {
        let (tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || imp::run_message_loop(tx));
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(InputHooks {
                thread: Some(thread),
                thread_id,
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err(InputHooksError::ThreadDiedBeforeReady)
            }
        }
    }

    #[cfg(not(windows))]
    pub fn start() -> Result<Self, InputHooksError> {
        Err(InputHooksError::ThreadDiedBeforeReady)
    }

    pub fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            #[cfg(windows)]
            imp::post_quit(self.thread_id);
            let _ = thread.join();
        }
    }
}

impl Drop for InputHooks {
    fn drop(&mut self) {
        self.stop();
    }
}
