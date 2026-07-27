//! Minimal local configuration — deliberately NOT a port of
//! `agent/core/config.py`'s full `config.yaml` schema (consent toggles,
//! category overrides, browser title rules, etc.) — that is a separate,
//! much larger concern than "does the installer/packaging story work,"
//! which is this task's actual scope. What's here is exactly the two
//! things this binary cannot function without: where to upload to, and
//! what to authenticate with.
//!
//! Defaults are chosen by `#[cfg(debug_assertions)]`, i.e. by Cargo's
//! build profile — NOT by an env var or a separate config file, so it's
//! structurally impossible to forget: `cargo run`/`cargo test`/plain
//! `cargo build` (debug profile) default to localhost, matching
//! `agent/config.yaml`'s own dev defaults, for local development against
//! a local backend+frontend. `cargo build --release` — what every real
//! installer/package (`installer/windows/agent.iss`, `installer/linux/
//! deb`, `installer/linux/rpm`, `installer/linux/tarball`, `installer/
//! linux/arch/PKGBUILD`) actually invokes — defaults to the real
//! production endpoints instead, so a freshly installed client talks to
//! devpace.ru out of the box, without anyone needing to hand-edit
//! `config.json` after install (see git history: that was the actual,
//! real gap that prompted this).
//!
//! A real deployment can still override either value by writing its own
//! `config.json` into the data directory (see `paths::data_dir`) before
//! first launch — these are only the values used when no such file (or
//! no such field in it) exists yet.
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
    /// See module doc comment — debug builds default to the local-dev
    /// placeholder, release builds (what every real installer produces)
    /// default to the real production API.
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

#[cfg(debug_assertions)]
fn default_backend_url() -> String {
    "http://localhost:8000".to_string()
}

#[cfg(not(debug_assertions))]
fn default_backend_url() -> String {
    "https://api.devpace.ru".to_string()
}

#[cfg(debug_assertions)]
fn default_dashboard_url() -> String {
    "http://localhost:5173".to_string()
}

#[cfg(not(debug_assertions))]
fn default_dashboard_url() -> String {
    "https://devpace.ru".to_string()
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

/// Production domains this binary has pointed to in the past, since
/// retired — astrakey.ru was the original (and briefly sole) production
/// domain before devpace.ru was acquired and promoted to primary
/// (2026-07-21). Any `config.json` written before that switch has these
/// values baked in from a real pairing session (see `persist_agent_token`
/// doc comment — an update never touches `backend_url`/`dashboard_url`
/// once written), so bumping the compiled default alone does nothing for
/// an already-installed, already-paired agent: it just keeps reading its
/// own stale file forever. Found on a real running install, not
/// hypothetical — see CHANGELOG.
const RETIRED_BACKEND_URLS: &[&str] = &["https://api.astrakey.ru"];
const RETIRED_DASHBOARD_URLS: &[&str] = &["https://astrakey.ru"];

/// Reads `config.json` from the data directory; missing file or
/// unparsable content both fall back to [`Config::default`] rather than
/// failing startup — a corrupt/missing local config must never stop the
/// agent from at least collecting and queuing locally.
///
/// Also self-heals a retired domain left over from before a production
/// domain switch (see [`RETIRED_BACKEND_URLS`]/[`RETIRED_DASHBOARD_URLS`])
/// by rewriting it to the current compiled default and persisting the
/// fix back to disk immediately — a user should never need to reinstall
/// or hand-edit `config.json` just because the product's own domain
/// moved.
pub fn load() -> Config {
    let path = paths::data_dir().join("config.json");
    let mut config: Config = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => return Config::default(),
    };

    let mut migrated = false;
    if RETIRED_BACKEND_URLS.contains(&config.backend_url.as_str()) {
        config.backend_url = default_backend_url();
        migrated = true;
    }
    if RETIRED_DASHBOARD_URLS.contains(&config.dashboard_url.as_str()) {
        config.dashboard_url = default_dashboard_url();
        migrated = true;
    }
    if migrated {
        let tmp_path = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(&config) {
            let _ = std::fs::write(&tmp_path, json).and_then(|_| std::fs::rename(&tmp_path, &path));
        }
    }

    config
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
    use std::sync::Mutex;

    // Rust runs tests in this file concurrently by default, but every test
    // that points `paths::data_dir()` at a scratch dir does so by mutating
    // the SAME process-wide env var (LOCALAPPDATA/XDG_DATA_HOME) — without
    // serializing those tests, one can clobber another's env var mid-run
    // (real failure seen: a `left == right` mismatch AND a Windows
    // "Отказано в доступе" from two tests racing on directory setup at
    // once). Any test that touches that env var must hold this lock for
    // its whole body.
    static ENV_VAR_LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = ENV_VAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn load_migrates_a_retired_domain_and_persists_the_fix_preserving_the_token() {
        let _guard = ENV_VAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = std::env::temp_dir().join(format!(
            "gla-config-migrate-test-{}-{}",
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

        std::fs::create_dir_all(paths::data_dir()).unwrap();
        let config_path = paths::data_dir().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"backend_url":"https://api.astrakey.ru","agent_token":"real-token","dashboard_url":"https://astrakey.ru"}"#,
        )
        .unwrap();

        let migrated = load();
        let on_disk_after = std::fs::read_to_string(&config_path).unwrap();

        match previous {
            Some(value) => std::env::set_var(env_var, value),
            None => std::env::remove_var(env_var),
        }
        std::fs::remove_dir_all(&scratch).ok();

        assert_eq!(migrated.backend_url, default_backend_url());
        assert_eq!(migrated.dashboard_url, default_dashboard_url());
        assert_eq!(migrated.agent_token.expose(), "real-token");
        // The fix must actually be written back, not just applied in memory --
        // otherwise every single launch re-does the same one-time migration.
        assert!(!on_disk_after.contains("astrakey"));
    }
}
