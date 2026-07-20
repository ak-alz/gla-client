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
//! for the Python MVP today. No pairing UI/flow is implemented here.

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
    fn debug_formatting_of_the_whole_config_never_reveals_the_real_token() {
        let config: Config =
            serde_json::from_str(r#"{"agent_token": "super-secret-abc123"}"#).unwrap();
        let formatted = format!("{config:?}");
        assert!(!formatted.contains("super-secret-abc123"));
    }
}
