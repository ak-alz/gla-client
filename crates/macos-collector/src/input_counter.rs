//! UNVERIFIED — see crate-level doc comment.

use crate::permissions::MissingPermission;
use objc2_core_foundation::{kCFRunLoopCommonModes, CFMachPort, CFRunLoop};
use objc2_core_graphics::{
    CGEvent, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
};
use std::sync::atomic::{AtomicI64, AtomicPtr, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

// Plain process-wide statics, not fields on `InputCounter` — required by
// the same constraint `windows-collector::hooks` already documents:
// `CGEventTapCallBack` is a bare `unsafe extern "C-unwind" fn` pointer
// with no per-registration user-data slot actually usable from safe
// Rust here (the `user_info: *mut c_void` parameter exists, but a
// process-wide `static` is the exact pattern already proven correct in
// this codebase for the structurally identical WH_KEYBOARD_LL/
// WH_MOUSE_LL case — reusing it rather than inventing a second way to
// solve the same problem).
static KEYBOARD_EVENTS: AtomicI64 = AtomicI64::new(0);
static MOUSE_MOVE_EVENTS: AtomicI64 = AtomicI64::new(0);
static MOUSE_CLICK_EVENTS: AtomicI64 = AtomicI64::new(0);

// Set right after a successful `tap_create` (before `tap_enable`), read
// by `tap_callback` to re-enable a tap the OS disabled for being too
// slow — NOT threaded through the `user_info: *mut c_void` parameter,
// since that would need the tap's own pointer before `tap_create` has
// returned it (a chicken-and-egg problem this static sidesteps, the
// same reason the event counters above are statics rather than fields).
static CURRENT_TAP: AtomicPtr<CFMachPort> = AtomicPtr::new(std::ptr::null_mut());

// `CGEventMaskBit(type)` is a C macro (`(CGEventMask)1 << type`), not an
// exported symbol any objc2-core-graphics version binds — computed
// directly from the real `CGEventType` values (confirmed against the
// actual cached 0.3.2 source, not guessed): KeyDown=10, MouseMoved=5,
// LeftMouseDown=1, RightMouseDown=3, OtherMouseDown=25 (macOS's own
// three-button convention, matching `windows-collector::hooks`' WM_L/M/R
// BUTTONDOWN triplet for "mouse click").
const EVENT_MASK: u64 = (1u64 << CGEventType::KeyDown.0)
    | (1u64 << CGEventType::MouseMoved.0)
    | (1u64 << CGEventType::LeftMouseDown.0)
    | (1u64 << CGEventType::RightMouseDown.0)
    | (1u64 << CGEventType::OtherMouseDown.0);

/// The tap callback — counts only, never reads/stores event content (no
/// keystrokes, no coordinates), matching `windows-collector`'s hook
/// procs and `linux-collector`'s evdev reader's exact same privacy
/// contract. Must always return the event unmodified (a listen-only tap
/// technically has its return value ignored by the OS, but returning it
/// as-is is the documented, correct convention regardless).
///
/// Also handles `kCGEventTapDisabledByTimeout`/`...ByUserInput` — macOS
/// disables a tap it judges too slow or that the user disabled from
/// Accessibility settings; re-enabling on timeout is the standard,
/// documented self-healing idiom for exactly this event tap pattern (not
/// scope creep — every real CGEventTap implementation includes it).
unsafe extern "C-unwind" fn tap_callback(
    _proxy: objc2_core_graphics::CGEventTapProxy,
    event_type: CGEventType,
    event: std::ptr::NonNull<CGEvent>,
    _user_info: *mut std::ffi::c_void,
) -> *mut CGEvent {
    match event_type {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            let tap_ptr = CURRENT_TAP.load(Ordering::Relaxed);
            if !tap_ptr.is_null() {
                let tap = unsafe { &*tap_ptr };
                CGEvent::tap_enable(tap, true);
            }
        }
        CGEventType::KeyDown => {
            KEYBOARD_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
        CGEventType::MouseMoved => {
            MOUSE_MOVE_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            MOUSE_CLICK_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
    event.as_ptr()
}

#[derive(Debug)]
pub enum InputCounterError {
    TapCreateFailed,
    RunLoopSourceCreateFailed,
}

impl std::fmt::Display for InputCounterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TapCreateFailed => write!(f, "CGEventTapCreate returned null"),
            Self::RunLoopSourceCreateFailed => write!(f, "CFMachPortCreateRunLoopSource returned null"),
        }
    }
}

impl std::error::Error for InputCounterError {}

/// A raw `CFRunLoop*`, sent across the thread boundary so `stop()` can
/// call `CFRunLoopStop` on it from the controlling thread. `CFRetained<
/// CFRunLoop>` is not `Send` (CF objects aren't assumed thread-safe by
/// default) — but `CFRunLoopStop` is Apple's own documented exception:
/// explicitly specified as safe to call from any thread, targeting a
/// run loop reference obtained via `CFRunLoopGetCurrent()` on the thread
/// that owns it (obtained once, below, right as that thread starts).
/// This wrapper exists ONLY to cross that one specific, documented-safe
/// boundary — never dereferenced except via `CFRunLoop::stop`.
struct RunLoopHandle(*const CFRunLoop);
unsafe impl Send for RunLoopHandle {}

/// Keyboard/mouse event counts since the last `take_and_reset()` — the
/// input-count capability, gated behind Accessibility permission (see
/// `permissions::accessibility_trusted`). Per this task's scoping
/// ("input aggregate only if permitted and necessary"), `start()` must
/// be an explicit opt-in the caller decides to attempt, not something
/// tried unconditionally at collector startup alongside
/// `active_app`/`idle` (which need no permission at all) — conflating
/// the two would make a single "no thanks" from the user look like it
/// blocks capabilities that don't actually need Accessibility.
pub struct InputCounter {
    thread: Option<JoinHandle<()>>,
    run_loop: Option<RunLoopHandle>,
}

impl InputCounter {
    pub fn new() -> Self {
        Self {
            thread: None,
            run_loop: None,
        }
    }

    /// Attempts to install a system-wide passive event tap
    /// (`CGEventTapCreate`) purely to COUNT events, never to read/store
    /// their content. Returns `Err(MissingPermission::
    /// AccessibilityNotGranted)` if `permissions::accessibility_trusted()`
    /// is false — checked BEFORE attempting the tap (this task's
    /// "graceful degraded mode" acceptance criterion needs the caller to
    /// distinguish "not granted, keep working without this" from an
    /// actual runtime error).
    ///
    /// A tap only actually delivers events while a `CFRunLoop` is
    /// running on the SAME thread it was created on — mirrors
    /// `windows-collector::hooks`' real Win32 message-pump requirement
    /// exactly (a real OS constraint on both platforms, not a design
    /// choice), so this spawns its own thread the same way, reporting
    /// readiness (or a startup failure) back via an `mpsc` channel
    /// before `start()` returns.
    ///
    /// UNVERIFIED: written against the real, cached `objc2-core-graphics`/
    /// `objc2-core-foundation` 0.3.2 sources (`CGEvent::tap_create`,
    /// `CGEvent::tap_enable`, `CFMachPort::new_run_loop_source`,
    /// `CFRunLoop::current`/`run`/`stop`/`add_source`, all confirmed to
    /// exist with these exact signatures) — but never compiled, linked,
    /// or run on a real Mac. Whether a `kCGEventTapOptionListenOnly` tap
    /// genuinely never blocks/delays real input delivery, and whether
    /// the process-wide `static` counters are visible correctly from the
    /// polling thread, are both believed correct (matching documented
    /// Apple behavior and the already-proven `windows-collector` static-
    /// counter pattern respectively) but unverified end to end.
    pub fn start(&mut self) -> Result<(), MissingPermission> {
        if !crate::permissions::accessibility_trusted() {
            return Err(MissingPermission::AccessibilityNotGranted);
        }

        let (ready_tx, ready_rx) = mpsc::channel::<Result<RunLoopHandle, InputCounterError>>();

        let thread = std::thread::spawn(move || {
            let tap = unsafe {
                CGEvent::tap_create(
                    CGEventTapLocation::HIDEventTap,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    EVENT_MASK,
                    Some(tap_callback),
                    std::ptr::null_mut(),
                )
            };
            let tap = match tap {
                Some(tap) => tap,
                None => {
                    let _ = ready_tx.send(Err(InputCounterError::TapCreateFailed));
                    return;
                }
            };

            let source = CFMachPort::new_run_loop_source(None, Some(&*tap), 0);
            let source = match source {
                Some(source) => source,
                None => {
                    let _ = ready_tx.send(Err(InputCounterError::RunLoopSourceCreateFailed));
                    return;
                }
            };

            let run_loop = match CFRunLoop::current() {
                Some(rl) => rl,
                None => {
                    let _ = ready_tx.send(Err(InputCounterError::RunLoopSourceCreateFailed));
                    return;
                }
            };
            run_loop.add_source(Some(&*source), unsafe { kCFRunLoopCommonModes });
            // Published BEFORE `tap_enable` so `tap_callback`'s timeout-
            // recovery branch can never observe a null pointer once
            // events could actually start arriving.
            CURRENT_TAP.store(&*tap as *const CFMachPort as *mut CFMachPort, Ordering::Relaxed);
            CGEvent::tap_enable(&tap, true);

            // Raw pointer, not the `CFRetained` handle itself — see
            // `RunLoopHandle`'s doc comment for why this specific crossing
            // is the one documented-safe exception.
            let run_loop_ptr = RunLoopHandle(&*run_loop as *const CFRunLoop);
            if ready_tx.send(Ok(run_loop_ptr)).is_err() {
                // Caller already gave up — tear down and exit rather than
                // pumping a run loop nobody will ever stop.
                CURRENT_TAP.store(std::ptr::null_mut(), Ordering::Relaxed);
                CGEvent::tap_enable(&tap, false);
                tap.invalidate();
                return;
            }

            // Blocks this thread, delivering tap callbacks, until
            // `stop()` calls `CFRunLoopStop` on `run_loop_ptr` from the
            // controlling thread.
            CFRunLoop::run();

            CURRENT_TAP.store(std::ptr::null_mut(), Ordering::Relaxed);
            CGEvent::tap_enable(&tap, false);
            tap.invalidate();
        });

        match ready_rx.recv() {
            Ok(Ok(run_loop)) => {
                self.thread = Some(thread);
                self.run_loop = Some(run_loop);
                Ok(())
            }
            Ok(Err(_)) | Err(_) => {
                // A real startup failure (tap/run-loop-source creation),
                // not a missing-permission case (already checked above) —
                // treat the same as "capability unavailable this session"
                // rather than propagating a distinct error type the
                // caller (`MacosSignalCollector::start`) has no use for
                // beyond "did input counting come up or not."
                let _ = thread.join();
                Err(MissingPermission::AccessibilityNotGranted)
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(run_loop) = self.run_loop.take() {
            // SAFETY: see `RunLoopHandle`'s doc comment — `CFRunLoopStop`
            // is Apple's documented thread-safe exception, called here
            // against the exact `CFRunLoopGetCurrent()` reference that
            // thread obtained for itself in `start()`.
            unsafe { (*run_loop.0).stop() };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    pub fn take_and_reset(&mut self) -> (i64, i64, i64) {
        (
            KEYBOARD_EVENTS.swap(0, Ordering::Relaxed),
            MOUSE_MOVE_EVENTS.swap(0, Ordering::Relaxed),
            MOUSE_CLICK_EVENTS.swap(0, Ordering::Relaxed),
        )
    }
}

impl Default for InputCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for InputCounter {
    fn drop(&mut self) {
        self.stop();
    }
}
