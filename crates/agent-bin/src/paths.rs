//! Per-user data directory layout — deliberately separate from the
//! install directory (`%LOCALAPPDATA%\Programs\GrowthLayerAgent\` on
//! Windows, `/opt/growth-layer-agent`/`/usr/bin` on Linux, wherever the
//! installer places the executable) so an upgrade/reinstall, which only
//! touches the install directory, structurally cannot disturb the
//! device pairing identity or the offline queue. This split is exactly
//! what makes "Pairing survives update"/"Queue survives update" true by
//! construction rather than by installer-script discipline alone — see
//! `agent-core/installer/windows/agent.iss` and
//! `agent-core/installer/linux/`'s doc comments for each installer's
//! half of this argument.

use std::path::PathBuf;

/// `%LOCALAPPDATA%\GrowthLayerAgent\` on Windows, `$XDG_DATA_HOME/
/// growth-layer-agent` (falling back to `~/.local/share/growth-layer-agent`
/// per the XDG Base Directory spec, the standard convention already used
/// for `~/.config/systemd/user/` in `lifecycle::Autostart`'s Linux path)
/// on Linux — never inside the install directory, never touched by the
/// installer at all (uninstall does not remove it by default — see each
/// installer script for that decision).
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));

    let base = base.unwrap_or_else(std::env::temp_dir); // extremely defensive fallback only

    #[cfg(windows)]
    let name = "GrowthLayerAgent";
    #[cfg(not(windows))]
    let name = "growth-layer-agent";

    base.join(name)
}

pub fn queue_dir() -> PathBuf {
    data_dir().join("queue")
}

pub fn device_id_path() -> PathBuf {
    data_dir().join("device_id.json")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn single_instance_lock_path() -> PathBuf {
    data_dir().join("agent.lock")
}

pub fn crash_marker_path() -> PathBuf {
    data_dir().join("crash_marker.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_paths_live_under_the_same_data_dir() {
        let root = data_dir();
        assert!(queue_dir().starts_with(&root));
        assert!(device_id_path().starts_with(&root));
        assert!(log_dir().starts_with(&root));
        assert!(single_instance_lock_path().starts_with(&root));
        assert!(crash_marker_path().starts_with(&root));
    }
}
