//! Per-user data directory layout — deliberately separate from the
//! install directory (`%LOCALAPPDATA%\Programs\GrowthLayerAgent\`, where
//! the installer places the executable) so an upgrade/reinstall, which
//! only touches the install directory, structurally cannot disturb the
//! device pairing identity or the offline queue. This split is exactly
//! what makes "Pairing survives update"/"Queue survives update" true by
//! construction rather than by installer-script discipline alone — see
//! `agent-core/installer/windows/agent.iss`'s doc comment for the
//! installer-side half of this argument.

use std::path::PathBuf;

/// `%LOCALAPPDATA%\GrowthLayerAgent\` — never inside the install
/// directory, never touched by the installer at all (uninstall does not
/// remove it by default — see the installer script for that decision).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir); // extremely defensive fallback only
    base.join("GrowthLayerAgent")
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
