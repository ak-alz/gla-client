//! Background download with bandwidth restraint and checksum
//! verification. `DownloadTransport` is a trait (not a hardcoded
//! `ureq` call), mirroring `uploader::UploadTransport`'s already-
//! established pattern — `MockDownloadTransport` drives every real
//! test in this module (including simulated network loss), the same
//! way `uploader::MockTransport` already does for fault injection,
//! without needing a live server.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checksum mismatch: expected {expected}, got {actual} — downloaded file discarded, not applied")]
    ChecksumMismatch { expected: String, actual: String },
}

pub trait DownloadTransport {
    /// Returns a reader over the artifact's bytes. Real errors
    /// (network loss, timeout, 404) surface as `Err` from the reader's
    /// own `read()` calls partway through, not just from this initial
    /// call — `download_with_checksum` below must handle both.
    fn get(&self, url: &str) -> Result<Box<dyn Read>, String>;
}

/// The real transport — `ureq`, same minimal blocking HTTP client
/// `uploader::UreqTransport` already uses (no async runtime, per
/// ADR 0013's resource-budget goal).
pub struct UreqDownloadTransport {
    agent: ureq::Agent,
}

impl UreqDownloadTransport {
    pub fn new(request_timeout: Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new().timeout(request_timeout).build(),
        }
    }
}

impl Default for UreqDownloadTransport {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

impl DownloadTransport for UreqDownloadTransport {
    fn get(&self, url: &str) -> Result<Box<dyn Read>, String> {
        let response = self
            .agent
            .get(url)
            .call()
            // Deliberately not preserving ureq::Error's Debug output
            // past this point (same reasoning as
            // uploader::transport::UreqTransport's doc comment: it can
            // embed the request URL, which this crate's callers must
            // not need to log at this layer).
            .map_err(|_| "download request failed".to_string())?;
        Ok(Box::new(response.into_reader()))
    }
}

#[derive(Default)]
pub struct DownloadConfig {
    /// `None` = no throttling. `Some(n)` = at most `n` bytes/second,
    /// enforced by sleeping out the remainder of any 1-second window
    /// in which more than `n` bytes were already read — approximate,
    /// not a precise token bucket, but sufficient for "don't saturate
    /// the user's connection," which is what "bandwidth restraint"
    /// asks for, not a traffic-shaping guarantee.
    pub max_bytes_per_second: Option<u64>,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `url` to `dest`, verifying its SHA-256 against
/// `expected_sha256_hex` (case-insensitive) before the file is
/// considered real. Writes to `dest` with a `.part` extension first,
/// only renaming (atomic on the same filesystem) into `dest` itself
/// after the checksum matches — a caller that sees `dest` exist can
/// trust it's the complete, verified artifact; a failed/interrupted
/// download never leaves a corrupt or partial file at `dest`.
///
/// Deliberately does NOT attempt to resume a previous `.part` file —
/// starts fresh on every call. Real resumability (HTTP `Range`
/// requests) would need the transport to support them and a way to
/// verify a partial file's prefix is genuinely a prefix of the same
/// artifact (not a stale download of a different, same-named
/// artifact) — added complexity "background download" doesn't
/// require, and simplicity here removes a whole class of partial-data
/// bugs a resumable version would need to guard against.
pub fn download_with_checksum(
    transport: &dyn DownloadTransport,
    url: &str,
    expected_sha256_hex: &str,
    dest: &Path,
    config: &DownloadConfig,
) -> Result<(), DownloadError> {
    let mut reader = transport.get(url).map_err(DownloadError::Transport)?;

    let part_path = dest.with_extension("part");
    let mut file = std::fs::File::create(&part_path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut bytes_this_window: u64 = 0;
    let mut window_start = Instant::now();

    let result = (|| -> Result<(), DownloadError> {
        loop {
            // Bounded to the remaining allowance in the current
            // throttle window — otherwise a single `read()` call
            // returning more than the whole per-second cap in one shot
            // (a real possibility, not just a test artifact: nothing
            // guarantees a `Read` impl hands back small chunks) would
            // silently skip the sleep this same iteration should have
            // triggered.
            let read_len = match config.max_bytes_per_second {
                Some(limit) => {
                    (limit.saturating_sub(bytes_this_window).max(1) as usize).min(buf.len())
                }
                None => buf.len(),
            };
            let n = reader.read(&mut buf[..read_len])?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;

            if let Some(limit) = config.max_bytes_per_second {
                bytes_this_window += n as u64;
                if bytes_this_window >= limit {
                    let elapsed = window_start.elapsed();
                    if elapsed < Duration::from_secs(1) {
                        std::thread::sleep(Duration::from_secs(1) - elapsed);
                    }
                    bytes_this_window = 0;
                    window_start = Instant::now();
                }
            }
        }
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = result {
        drop(file);
        std::fs::remove_file(&part_path).ok();
        return Err(err);
    }
    drop(file);

    let actual = to_hex(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256_hex) {
        std::fs::remove_file(&part_path).ok();
        return Err(DownloadError::ChecksumMismatch {
            expected: expected_sha256_hex.to_string(),
            actual,
        });
    }

    std::fs::rename(&part_path, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sha256_hex(data: &[u8]) -> String {
        to_hex(&Sha256::digest(data))
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "updater-download-test-{name}-{}",
            std::process::id()
        ))
    }

    struct MockDownloadTransport {
        bytes: Vec<u8>,
        /// If `Some(n)`, the reader fails with an error after `n` bytes
        /// have been successfully read — simulates a network drop
        /// partway through a real download.
        fail_after_bytes: Option<usize>,
    }

    struct FlakyReader {
        data: Cursor<Vec<u8>>,
        bytes_read: usize,
        fail_after_bytes: Option<usize>,
    }

    impl Read for FlakyReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if let Some(limit) = self.fail_after_bytes {
                if self.bytes_read >= limit {
                    return Err(std::io::Error::other("simulated network loss"));
                }
            }
            let n = self.data.read(buf)?;
            self.bytes_read += n;
            Ok(n)
        }
    }

    impl DownloadTransport for MockDownloadTransport {
        fn get(&self, _url: &str) -> Result<Box<dyn Read>, String> {
            Ok(Box::new(FlakyReader {
                data: Cursor::new(self.bytes.clone()),
                bytes_read: 0,
                fail_after_bytes: self.fail_after_bytes,
            }))
        }
    }

    #[test]
    fn a_complete_download_with_a_matching_checksum_succeeds() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(100);
        let transport = MockDownloadTransport {
            bytes: data.clone(),
            fail_after_bytes: None,
        };
        let dest = temp_path("ok");
        let _ = std::fs::remove_file(&dest);

        download_with_checksum(
            &transport,
            "https://example.invalid/artifact",
            &sha256_hex(&data),
            &dest,
            &DownloadConfig::default(),
        )
        .expect("download must succeed");

        assert_eq!(std::fs::read(&dest).unwrap(), data);
        std::fs::remove_file(&dest).ok();
    }

    #[test]
    fn a_checksum_mismatch_is_rejected_and_leaves_no_file_at_dest() {
        let data = b"real artifact bytes".to_vec();
        let transport = MockDownloadTransport {
            bytes: data.clone(),
            fail_after_bytes: None,
        };
        let dest = temp_path("mismatch");
        let _ = std::fs::remove_file(&dest);

        let result = download_with_checksum(
            &transport,
            "https://example.invalid/artifact",
            &sha256_hex(b"different bytes entirely"),
            &dest,
            &DownloadConfig::default(),
        );

        assert!(matches!(
            result,
            Err(DownloadError::ChecksumMismatch { .. })
        ));
        assert!(
            !dest.exists(),
            "dest must not exist after a checksum mismatch"
        );
        assert!(
            !dest.with_extension("part").exists(),
            "the .part file must be cleaned up, not left behind"
        );
    }

    #[test]
    fn simulated_network_loss_partway_through_is_reported_and_leaves_no_file_at_dest() {
        let data = b"0123456789".repeat(1000); // 10,000 bytes
        let transport = MockDownloadTransport {
            bytes: data.clone(),
            fail_after_bytes: Some(4000), // drop the connection partway through
        };
        let dest = temp_path("network-loss");
        let _ = std::fs::remove_file(&dest);

        let result = download_with_checksum(
            &transport,
            "https://example.invalid/artifact",
            &sha256_hex(&data),
            &dest,
            &DownloadConfig::default(),
        );

        assert!(matches!(result, Err(DownloadError::Io(_))));
        assert!(!dest.exists());
        assert!(!dest.with_extension("part").exists());
    }

    #[test]
    fn bandwidth_restraint_actually_slows_a_download_down() {
        // 3 windows' worth of data at a 10,000 bytes/sec cap should take
        // at least ~2 full seconds (the first window is "free", each
        // subsequent one waits out its window) — a real wall-clock
        // check, not just "the parameter was accepted."
        let data = vec![0u8; 25_000];
        let transport = MockDownloadTransport {
            bytes: data.clone(),
            fail_after_bytes: None,
        };
        let dest = temp_path("throttled");
        let _ = std::fs::remove_file(&dest);

        let start = Instant::now();
        download_with_checksum(
            &transport,
            "https://example.invalid/artifact",
            &sha256_hex(&data),
            &dest,
            &DownloadConfig {
                max_bytes_per_second: Some(10_000),
            },
        )
        .expect("download must succeed");
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(1800),
            "expected throttling to take at least ~2s for 25KB at 10KB/s, took {elapsed:?}"
        );
        std::fs::remove_file(&dest).ok();
    }

    #[test]
    fn retrying_after_a_failed_download_starts_fresh_not_resumed() {
        let data = b"abcdefghij".repeat(500);
        let flaky = MockDownloadTransport {
            bytes: data.clone(),
            fail_after_bytes: Some(1000),
        };
        let dest = temp_path("retry");
        let _ = std::fs::remove_file(&dest);

        assert!(download_with_checksum(
            &flaky,
            "https://example.invalid/artifact",
            &sha256_hex(&data),
            &dest,
            &DownloadConfig::default(),
        )
        .is_err());

        let reliable = MockDownloadTransport {
            bytes: data.clone(),
            fail_after_bytes: None,
        };
        download_with_checksum(
            &reliable,
            "https://example.invalid/artifact",
            &sha256_hex(&data),
            &dest,
            &DownloadConfig::default(),
        )
        .expect("retry must succeed");

        assert_eq!(std::fs::read(&dest).unwrap(), data);
        std::fs::remove_file(&dest).ok();
    }
}
