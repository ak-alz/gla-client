use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use uuid::Uuid;

/// Unique per-record identifier. New on every envelope — this is what makes
/// upload idempotent/dedupable (AG-004), which the current Python MVP schema
/// has no equivalent of at all (see AGENT_EVENT_PARITY.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(Uuid);

impl EventId {
    pub fn new() -> Self {
        EventId(Uuid::new_v4())
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable per-installation identifier, persisted to disk so it survives
/// restarts. Distinguishes this from `EventId`: one device produces many
/// events, all sharing the same `device_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(Uuid);

impl DeviceId {
    pub fn from_uuid(id: Uuid) -> Self {
        DeviceId(id)
    }

    /// Loads the persisted id at `path`, creating and persisting a new one
    /// on first run (missing file) or if the file is present but unparsable
    /// (corrupt). A device_id that changed every process restart would defeat
    /// its own purpose (telling installations apart across restarts), so
    /// this must read back the same value it wrote — proven by a round-trip
    /// test, not just asserted.
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(raw) => match Uuid::parse_str(raw.trim()) {
                Ok(id) => Ok(DeviceId(id)),
                Err(_) => Self::create_and_persist(path),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::create_and_persist(path),
            Err(err) => Err(err),
        }
    }

    fn create_and_persist(path: &Path) -> io::Result<Self> {
        let id = Uuid::new_v4();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Write-then-rename: a crash mid-write must never leave a
        // half-written file that a later `load_or_create` misreads as a
        // corrupt (and therefore silently replaced) identity.
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, id.to_string())?;
        fs::rename(&tmp_path, path)?;
        Ok(DeviceId(id))
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_create_persists_across_calls() {
        let dir = std::env::temp_dir().join(format!("event-contract-test-{}", Uuid::new_v4()));
        let path = dir.join("device_id");

        let first = DeviceId::load_or_create(&path).expect("first load creates");
        let second = DeviceId::load_or_create(&path).expect("second load reads back");
        assert_eq!(
            first, second,
            "device_id must survive a re-read of the same path"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_or_create_recovers_from_corrupt_file() {
        let dir = std::env::temp_dir().join(format!("event-contract-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device_id");
        fs::write(&path, "not-a-uuid").unwrap();

        let recovered = DeviceId::load_or_create(&path);
        assert!(recovered.is_ok(), "corrupt file must not be a hard failure");

        fs::remove_dir_all(&dir).ok();
    }
}
