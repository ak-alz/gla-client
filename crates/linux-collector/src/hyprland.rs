//! Hyprland's own IPC socket (`hyprctl activewindow`, confirmed during
//! AG-LNX-001's research as the standard mechanism — `wiki.hypr.land/IPC`)
//! — the second real Wayland backend this task adds, chosen specifically
//! because it needs no external component (no Shell extension, no
//! loaded script) to write and ship, unlike GNOME/KDE (see
//! `environment.rs`'s doc comment for that scoping decision).
//!
//! # What could and could not be verified for real in this environment
//!
//! This crate's dev/verification environment (WSLg) runs Weston, not
//! Hyprland — there is no real Hyprland compositor to connect to here.
//! The socket-path construction and the actual Unix-socket client/JSON-
//! parsing code below ARE genuinely exercised for real in this crate's
//! own tests, against a real, temporary Unix socket server this test
//! starts itself (not a real `hyprctl`, but a real socket, a real
//! connection, and the exact same read/parse code a real Hyprland
//! session would drive) — the same "mock the transport, not the logic"
//! technique already used for `uploader::MockTransport` (AG-005).

use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HyprlandError {
    #[error("HYPRLAND_INSTANCE_SIGNATURE is not set — not running under Hyprland")]
    NotRunningUnderHyprland,
    #[error("XDG_RUNTIME_DIR is not set")]
    NoRuntimeDir,
    #[error("failed to connect to the Hyprland IPC socket: {0}")]
    Connect(std::io::Error),
    #[error("failed to communicate over the Hyprland IPC socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("Hyprland IPC returned unparsable JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Deserialize)]
struct ActiveWindowResponse {
    pid: Option<i64>,
}

/// `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock` —
/// the well-known, documented Hyprland IPC socket path.
pub fn socket_path(
    xdg_runtime_dir: Option<&str>,
    hyprland_instance_signature: Option<&str>,
) -> Result<PathBuf, HyprlandError> {
    let signature = hyprland_instance_signature
        .filter(|s| !s.is_empty())
        .ok_or(HyprlandError::NotRunningUnderHyprland)?;
    let runtime_dir = xdg_runtime_dir
        .filter(|s| !s.is_empty())
        .ok_or(HyprlandError::NoRuntimeDir)?;
    Ok(PathBuf::from(runtime_dir)
        .join("hypr")
        .join(signature)
        .join(".socket.sock"))
}

/// Sends `j/activewindow` (the `j/` prefix requests JSON output, per
/// Hyprland's IPC documentation) over the socket at `path` and returns
/// the active window's PID, or `None` if Hyprland reports no active
/// window (e.g. an empty workspace).
pub fn active_window_pid(path: &std::path::Path) -> Result<Option<u32>, HyprlandError> {
    let mut stream = UnixStream::connect(path).map_err(HyprlandError::Connect)?;
    stream.write_all(b"j/activewindow")?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;

    let parsed: ActiveWindowResponse = serde_json::from_str(&response)?;
    Ok(parsed.pid.and_then(|pid| u32::try_from(pid).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn socket_path_uses_the_documented_hyprland_layout() {
        let path = socket_path(Some("/run/user/1000"), Some("abc123")).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/run/user/1000/hypr/abc123/.socket.sock")
        );
    }

    #[test]
    fn socket_path_errors_when_not_under_hyprland() {
        assert!(matches!(
            socket_path(Some("/run/user/1000"), None),
            Err(HyprlandError::NotRunningUnderHyprland)
        ));
        assert!(matches!(
            socket_path(Some("/run/user/1000"), Some("")),
            Err(HyprlandError::NotRunningUnderHyprland)
        ));
    }

    #[test]
    fn socket_path_errors_without_runtime_dir() {
        assert!(matches!(
            socket_path(None, Some("abc123")),
            Err(HyprlandError::NoRuntimeDir)
        ));
    }

    /// Real Unix socket, real connection, real read/write — a genuine
    /// server this test starts itself, standing in for `hyprctl`'s
    /// backend (Hyprland's own compositor process) since no real
    /// Hyprland compositor exists in this dev environment (see module
    /// doc comment).
    #[test]
    fn active_window_pid_parses_a_real_socket_response() {
        let dir = std::env::temp_dir().join(format!(
            "linux-collector-hypr-test-{}-{}",
            std::process::id(),
            uuid_like_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let socket_path = dir.join(".socket.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut request = String::new();
            conn.read_to_string(&mut request).unwrap();
            assert_eq!(request, "j/activewindow");
            conn.write_all(br#"{"address":"0x1","pid":4242,"class":"foo"}"#)
                .unwrap();
        });

        let pid = active_window_pid(&socket_path).unwrap();
        assert_eq!(pid, Some(4242));

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn active_window_pid_handles_no_active_window() {
        let dir = std::env::temp_dir().join(format!(
            "linux-collector-hypr-test-noactive-{}-{}",
            std::process::id(),
            uuid_like_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let socket_path = dir.join(".socket.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut request = String::new();
            conn.read_to_string(&mut request).unwrap();
            conn.write_all(b"{}").unwrap();
        });

        let pid = active_window_pid(&socket_path).unwrap();
        assert_eq!(pid, None);

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn uuid_like_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }
}
