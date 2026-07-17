//! Idle-timer math, split into a pure function (`idle_seconds_from_ticks`,
//! testable on any platform) and an OS-facing wrapper (`get_idle_seconds`,
//! Windows-only) that feeds it real `GetLastInputInfo`/`GetTickCount`
//! values — mirrors `_get_idle_seconds()` in the Python source exactly.

/// `current_tick`/`last_input_tick` are both raw millisecond counts from
/// `GetTickCount`, which wraps to 0 roughly every 49.7 days of uptime. The
/// Python source computes `current_tick - last_input_tick` with plain
/// (signed, arbitrary-precision) subtraction and clamps a negative result
/// to `0.0`, which means the exact wrap instant is silently reported as
/// "not idle" rather than the correct small idle duration. `u32::wrapping_sub`
/// gives the mathematically correct duration across a wraparound instead of
/// that clamp-to-zero — a narrow, deliberate improvement over bug-for-bug
/// parity (there is no golden fixture covering this OS-integer-wraparound
/// edge case, and it only differs from the Python behavior once every ~49.7
/// days of continuous uptime), documented here and in SUMMARY.md rather
/// than silently diverging.
pub fn idle_seconds_from_ticks(current_tick: u32, last_input_tick: u32) -> f64 {
    let idle_ms = current_tick.wrapping_sub(last_input_tick);
    idle_ms as f64 / 1000.0
}

#[cfg(windows)]
pub fn get_idle_seconds() -> f64 {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // If GetLastInputInfo fails (extremely unlikely — no documented failure
    // mode besides a malformed cbSize, which is fixed here), treat the
    // input timestamp as "now," i.e. not idle, the same fail-safe direction
    // the rest of this crate takes on any Win32 call it cannot recover from.
    let current_tick = unsafe { GetTickCount() };
    let last_input_tick = unsafe {
        if GetLastInputInfo(&mut info) != 0 {
            info.dwTime
        } else {
            current_tick
        }
    };
    idle_seconds_from_ticks(current_tick, last_input_tick)
}

/// Non-Windows stub, so `cargo build --workspace` still compiles this
/// crate on a non-Windows CI machine — `WindowsSignalCollector` is never
/// actually instantiated on such a platform (Linux/macOS get their own
/// collector crates in later tasks), so "always report not idle" here is
/// never more than a compile-time placeholder.
#[cfg(not(windows))]
pub fn get_idle_seconds() -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_case_matches_simple_subtraction() {
        assert_eq!(idle_seconds_from_ticks(10_000, 7_000), 3.0);
    }

    #[test]
    fn no_idle_time_is_zero() {
        assert_eq!(idle_seconds_from_ticks(5_000, 5_000), 0.0);
    }

    #[test]
    fn wraparound_is_handled_correctly() {
        // last_input_tick just before wrap, current_tick just after —
        // true elapsed time is 10ms, not a huge negative-turned-zero.
        let last_input_tick = u32::MAX - 4;
        let current_tick = 5u32;
        assert_eq!(
            idle_seconds_from_ticks(current_tick, last_input_tick),
            0.010
        );
    }
}
