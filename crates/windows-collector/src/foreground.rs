//! Foreground window → process name, mirroring `_get_foreground_hwnd`/
//! `_get_process_name` in the Python source. `psutil.Process(pid).name()`
//! returns just the executable's file name (e.g. `"chrome.exe"`), never a
//! full path — `QueryFullProcessImageNameW` returns a full path, so the
//! Windows-only wrapper extracts the file name the same way `extract_file_name`
//! (below, pure and tested everywhere) does.
//!
//! Uses `PROCESS_QUERY_LIMITED_INFORMATION` (not the unrestricted
//! `PROCESS_QUERY_INFORMATION`) deliberately: it is the access right
//! Microsoft specifically documents as available to a standard,
//! non-elevated caller for querying another process's basic info, matching
//! this task's "no admin required" acceptance criterion. Like the Python
//! source's blanket `except (psutil.NoSuchProcess, psutil.AccessDenied,
//! Exception): return None`, any failure here (including `OpenProcess`
//! being denied for a foreground window running elevated, or as a
//! different user) is treated as "unknown process," not an error — the
//! collector's job is to degrade gracefully, never to require elevation.
//!
//! `foreground_hwnd()` and `process_name_for_hwnd(hwnd)` are deliberately
//! two calls, not one combined `foreground_process_name()` — `collector.rs`
//! calls `foreground_hwnd()` exactly once per poll and reuses that SAME
//! handle for both the process-name lookup and (for browsers only) the
//! title read in `browser_title.rs`. Calling `GetForegroundWindow` a
//! second time for the title read could observe a different window if the
//! foreground changed between the two calls — a narrow but real
//! consistency bug that a combined function would reintroduce.

/// Extracts the file-name component from a full path, matching what
/// `psutil.Process(pid).name()` returns from a full image path — pure,
/// platform-independent, and exercised directly by unit tests without
/// needing a real process handle.
pub fn extract_file_name(full_path: &str) -> Option<String> {
    std::path::Path::new(full_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Returned as `usize`, not the raw `HWND` pointer type, because a real
/// `HWND` (`*mut c_void`) is `!Send`/`!Sync` — every caller in this crate
/// that needs to move a window handle across a thread boundary (see
/// `native_loop.rs`) converts back to the pointer type only at the actual
/// Win32 call site.
#[cfg(windows)]
pub fn foreground_hwnd() -> Option<usize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        None
    } else {
        Some(hwnd as usize)
    }
}

#[cfg(not(windows))]
pub fn foreground_hwnd() -> Option<usize> {
    None
}

#[cfg(windows)]
pub fn process_name_for_hwnd(hwnd: usize) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HWND};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let hwnd = hwnd as HWND;
    let mut pid: u32 = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if thread_id == 0 || pid == 0 {
        return None;
    }

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None; // access denied (elevated/other-user process) or gone
    }

    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        return None;
    }

    let full_path = String::from_utf16_lossy(&buf[..size as usize]);
    extract_file_name(&full_path)
}

#[cfg(not(windows))]
pub fn process_name_for_hwnd(_hwnd: usize) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_file_name_from_windows_path() {
        assert_eq!(
            extract_file_name(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            Some("chrome.exe".to_string())
        );
    }

    #[test]
    fn bare_file_name_returns_itself() {
        assert_eq!(
            extract_file_name("chrome.exe"),
            Some("chrome.exe".to_string())
        );
    }

    #[test]
    fn empty_path_returns_none() {
        assert_eq!(extract_file_name(""), None);
    }
}
