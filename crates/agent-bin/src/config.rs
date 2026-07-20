//! Minimal local configuration — deliberately NOT a port of
//! `agent/core/config.py`'s full `config.yaml` schema (consent toggles,
//! category overrides, browser title rules, etc.) — that is a separate,
//! much larger concern than "does the installer/packaging story work,"
//! which is this task's actual scope. What's here is exactly the two
//! things this binary cannot function without: where to upload to, and
//! what to authenticate with. Both default to values that only work for
//! local development (matching `agent/config.yaml`'s own committed
//! defaults) — a real deployment is expected to write a real
//! `config.json` into the data directory (see `paths::data_dir`), the
//! same way `agent/core/pairing.py` writes `exports/device_credentials.json`
//! for the Python MVP today.
//!
//! The real device-authorization pairing flow (`pairing.rs`) persists a
//! newly-obtained `agent_token` back into this same file via
//! [`persist_agent_token`] — read-modify-write, preserving whatever
//! `backend_url`/`dashboard_url` the file already had.

use crate::paths;
use secrets::SecretString;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Matches `agent/config.yaml`'s own local-dev default exactly —
    /// this is not a production endpoint, just the same placeholder the
    /// Python MVP ships with.
    #[serde(default = "default_backend_url")]
    pub backend_url: String,
    /// Empty means "not yet paired" — the uploader will get a real,
    /// already-handled `CycleOutcome::Unauthorized` from the backend
    /// rather than crash (see `uploader::CycleOutcome`, from AG-005),
    /// so an unpaired agent still collects and queues locally without
    /// erroring. `SecretString` (AG-SEC-001) — `#[derive(Debug)]` on
    /// this struct can never accidentally print the real token value;
    /// serializes transparently as a bare string, so `config.json`'s
    /// on-disk shape is unchanged.
    #[serde(default)]
    pub agent_token: SecretString,
    #[serde(default = "default_dashboard_url")]
    pub dashboard_url: String,
}

fn default_backend_url() -> String {
    "http://localhost:8000".to_string()
}

fn default_dashboard_url() -> String {
    "http://localhost:5173".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            backend_url: default_backend_url(),
            agent_token: SecretString::new(""),
            dashboard_url: default_dashboard_url(),
        }
    }
}

/// Reads `config.json` from the data directory; missing file or
/// unparsable content both fall back to [`Config::default`] rather than
/// failing startup — a corrupt/missing local config must never stop the
/// agent from at least collecting and queuing locally.
pub fn load() -> Config {
    let path = paths::data_dir().join("config.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

/// Writes a newly-obtained `agent_token` back into `config.json`,
/// preserving whatever `backend_url`/`dashboard_url` are already there —
/// re-reads the current file (via [`load`], so a missing/corrupt file is
/// still handled the same defensive way) rather than assuming the
/// in-memory `Config` this process started with is still accurate, then
/// writes the whole struct back. Write-then-rename (same pattern
/// `DeviceId::create_and_persist` already uses) so a crash mid-write
/// never leaves a half-written `config.json` behind.
pub fn persist_agent_token(agent_token: &SecretString) -> std::io::Result<()> {
    let mut current = load();
    current.agent_token = agent_token.clone();
    let path = paths::data_dir().join("config.json");
    let tmp_path = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&current)
        .expect("Config serialization is infallible: no non-finite floats, all fields are plain owned data");
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_unpaired_with_local_dev_urls() {
        let config = Config::default();
        assert!(config.agent_token.is_empty());
        assert_eq!(config.backend_url, "http://localhost:8000");
        assert_eq!(config.dashboard_url, "http://localhost:5173");
    }

    #[test]
    fn deserializes_partial_json_filling_in_defaults() {
        let config: Config = serde_json::from_str(r#"{"agent_token": "abc123"}"#).unwrap();
        assert_eq!(config.agent_token.expose(), "abc123");
        assert_eq!(config.backend_url, "http://localhost:8000");
    }

    #[test]
    fn persist_agent_token_round_trips_and_preserves_other_fields() {
        // A real temp data dir, never the real user's -- overrides the
        // same env var `paths::data_dir()` reads, restored immediately
        // after so no other test in this binary sees it changed.
        let scratch = std::env::temp_dir().join(format!(
            "gla-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&scratch).unwrap();
        #[cfg(windows)]
        let env_var = "LOCALAPPDATA";
        #[cfg(not(windows))]
        let env_var = "XDG_DATA_HOME";
        let previous = std::env::var_os(env_var);
        std::env::set_var(env_var, &scratch);

        let seeded = Config {
            backend_url: "https://api.example.test".to_string(),
            agent_token: SecretString::new(""),
            dashboard_url: "https://app.example.test".to_string(),
        };
        // data_dir() only exists after main() creates it normally --
        // this test creates it itself first, the same way main() does.
        std::fs::create_dir_all(paths::data_dir()).unwrap();
        std::fs::write(
            paths::data_dir().join("config.json"),
            serde_json::to_string(&seeded).unwrap(),
        )
        .unwrap();

        persist_agent_token(&SecretString::new("real-token-from-real-pairing")).unwrap();
        let reloaded = load();

        match previous {
            Some(value) => std::env::set_var(env_var, value),
            None => std::env::remove_var(env_var),
        }
        std::fs::remove_dir_all(&scratch).ok();

        assert_eq!(reloaded.agent_token.expose(), "real-token-from-real-pairing");
        assert_eq!(reloaded.backend_url, "https://api.example.test");
        assert_eq!(reloaded.dashboard_url, "https://app.example.test");
    }

    #[test]
    fn debug_formatting_of_the_whole_config_never_reveals_the_real_token() {
        let config: Config =
            serde_json::from_str(r#"{"agent_token": "super-secret-abc123"}"#).unwrap();
        let formatted = format!("{config:?}");
        assert!(!formatted.contains("super-secret-abc123"));
    }
}
