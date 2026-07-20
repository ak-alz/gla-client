//! End-to-end: disk check -> download+checksum -> restart-policy gate
//! -> stage -> health check -> commit/rollback, in the order a real
//! caller (`agent-bin`) would use this crate — proves the pieces
//! compose, and specifically exercises both a "power loss during
//! staging" and a "network loss during download" scenario together in
//! one realistic narrative, matching AG-UPD-002's own "power/network
//! loss during update tested" acceptance criterion end to end rather
//! than only per-module.

use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use updater::{
    apply_with_health_check, download_with_checksum, has_enough_free_space, ApplyOutcome,
    DownloadConfig, DownloadError, DownloadTransport, HealthCheck, RestartContext, RestartPolicy,
    Staging,
};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "updater-integration-test-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct ReliableTransport(Vec<u8>);
impl DownloadTransport for ReliableTransport {
    fn get(&self, _url: &str) -> Result<Box<dyn Read>, String> {
        Ok(Box::new(Cursor::new(self.0.clone())))
    }
}

struct DroppedConnectionTransport;
impl DownloadTransport for DroppedConnectionTransport {
    fn get(&self, _url: &str) -> Result<Box<dyn Read>, String> {
        Err("connection reset by peer".to_string())
    }
}

struct FixedHealth(bool);
impl HealthCheck for FixedHealth {
    fn is_healthy(&self) -> bool {
        self.0
    }
}

#[test]
fn full_pipeline_applies_a_good_update_end_to_end() {
    let install_dir = temp_dir("full-pipeline-good");
    std::fs::write(install_dir.join("agent.bin"), b"v1.0.0 binary").unwrap();

    let new_version = b"v2.0.0 binary, genuinely healthy".to_vec();
    let checksum = sha256_hex(&new_version);
    let transport = ReliableTransport(new_version.clone());
    let downloaded_path = install_dir.join("downloaded_v2");

    // Step 1: disk check.
    assert!(has_enough_free_space(&install_dir, 1).unwrap());

    // Step 2: download + checksum.
    download_with_checksum(
        &transport,
        "https://example.invalid/v2.0.0",
        &checksum,
        &downloaded_path,
        &DownloadConfig::default(),
    )
    .expect("download must succeed");
    assert_eq!(std::fs::read(&downloaded_path).unwrap(), new_version);

    // Step 3: restart policy — user explicitly confirmed a restart.
    let policy = RestartPolicy::default();
    let ctx = RestartContext {
        mandatory: false,
        user_explicitly_confirmed: true,
        session_ending: false,
        idle_seconds: 0.0,
    };
    assert!(policy.should_restart_now(&ctx));

    // Step 4: stage + health check + commit.
    let staging = Staging::new(&install_dir, "agent.bin");
    let outcome = apply_with_health_check(&staging, &downloaded_path, &FixedHealth(true)).unwrap();

    assert_eq!(outcome, ApplyOutcome::Applied);
    assert_eq!(
        std::fs::read(install_dir.join("agent.bin")).unwrap(),
        new_version
    );
    assert!(!staging.has_pending_interrupted_apply());

    std::fs::remove_dir_all(&install_dir).ok();
}

#[test]
fn network_loss_during_download_never_reaches_staging_and_old_version_keeps_running() {
    let install_dir = temp_dir("network-loss-narrative");
    std::fs::write(install_dir.join("agent.bin"), b"v1.0.0, still running").unwrap();

    let transport = DroppedConnectionTransport;
    let downloaded_path = install_dir.join("downloaded_v2");

    let result = download_with_checksum(
        &transport,
        "https://example.invalid/v2.0.0",
        &sha256_hex(b"whatever v2.0.0 would have been"),
        &downloaded_path,
        &DownloadConfig::default(),
    );

    assert!(matches!(result, Err(DownloadError::Transport(_))));
    assert!(!downloaded_path.exists());
    // The old binary was never touched — staging never even started.
    assert_eq!(
        std::fs::read(install_dir.join("agent.bin")).unwrap(),
        b"v1.0.0, still running"
    );

    std::fs::remove_dir_all(&install_dir).ok();
}

#[test]
fn power_loss_immediately_after_staging_is_recovered_by_rolling_back_on_next_startup() {
    let install_dir = temp_dir("power-loss-narrative");
    std::fs::write(install_dir.join("agent.bin"), b"v1.0.0, known-good").unwrap();

    let new_version = b"v2.0.0, never got a health check".to_vec();
    let downloaded_path = install_dir.join("downloaded_v2");
    std::fs::write(&downloaded_path, &new_version).unwrap();

    // The real process would call apply_with_health_check() here, but
    // to simulate power loss BETWEEN staging and the health check, we
    // call stage_and_swap() directly and never reach commit/rollback —
    // representing the process dying right there.
    {
        let staging = Staging::new(&install_dir, "agent.bin");
        staging.stage_and_swap(&downloaded_path).unwrap();
        assert_eq!(
            std::fs::read(install_dir.join("agent.bin")).unwrap(),
            new_version
        );
        // <-- "power loss" here: no commit(), no rollback() call.
    }

    // Next startup: a fresh Staging instance, as a real restart would construct.
    let restarted_staging = Staging::new(&install_dir, "agent.bin");
    assert!(
        restarted_staging.has_pending_interrupted_apply(),
        "startup must detect the apply that never confirmed its own health"
    );
    // Correct, safe default: never assume an unconfirmed update is good.
    restarted_staging.rollback().unwrap();
    assert_eq!(
        std::fs::read(install_dir.join("agent.bin")).unwrap(),
        b"v1.0.0, known-good"
    );

    std::fs::remove_dir_all(&install_dir).ok();
}
