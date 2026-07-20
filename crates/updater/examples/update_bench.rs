//! AG-PERF-001's `update_download`/`update_apply` benchmark scenarios need
//! a real, standalone process exercising the actual `updater` crate code
//! paths (`download_with_checksum`, `Staging::stage_and_swap`/`commit`) so
//! `run.ps1`/`run.sh` can measure its real RSS/CPU the same way they
//! already measure `growth-layer-agent` itself for every other scenario —
//! rather than trusting the crate's existing unit tests (which use
//! `MockDownloadTransport`, deliberately never a live socket) to stand in
//! for a real resource measurement.
//!
//! Usage: `update_bench download <artifact_bytes> <max_bytes_per_second>`
//!        `update_bench apply`

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;
use updater::{download_with_checksum, DownloadConfig, Staging, UreqDownloadTransport};

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Deterministic, non-repeating-enough-to-matter fill -- this is a
/// resource-usage benchmark, not a test of randomness quality, and a
/// dependency-free fill keeps this example's own footprint minimal.
fn fill_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 256) as u8).collect()
}

fn run_download(artifact_bytes: usize, max_bytes_per_second: Option<u64>) {
    // `Arc`, not `.clone()` of the `Vec` itself -- this benchmark measures
    // the DOWNLOADER's own overhead, not this harness's own setup code, so
    // the server thread must share the same backing bytes rather than
    // doubling this process's RSS with a second full copy (a real,
    // self-inflicted inflation found by an actual run: `update_download`'s
    // real budget violation turned out to be this benchmark counting its
    // own redundant copy, not the downloader itself).
    let body = Arc::new(fill_bytes(artifact_bytes));
    let sha256_hex = to_hex(&Sha256::digest(body.as_slice()));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let server_body = Arc::clone(&body);
    std::thread::spawn(move || {
        // One real, minimal, blocking HTTP/1.0 response -- no framework
        // needed for "serve this exact byte blob once."
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf); // discard the request line/headers
            let header = format!(
                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                server_body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&server_body);
        }
    });

    let dest = std::env::temp_dir().join(format!("update-bench-artifact-{}.bin", std::process::id()));
    let transport = UreqDownloadTransport::new(Duration::from_secs(30));
    let config = DownloadConfig {
        max_bytes_per_second,
    };
    let url = format!("http://127.0.0.1:{port}/artifact");

    download_with_checksum(&transport, &url, &sha256_hex, &dest, &config)
        .expect("download_with_checksum must succeed against this benchmark's own real server/checksum");

    println!(
        "downloaded {} bytes to {} (checksum verified)",
        artifact_bytes,
        dest.display()
    );
    let _ = std::fs::remove_file(&dest);
}

fn run_apply() {
    let scratch = std::env::temp_dir().join(format!("update-bench-apply-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("create scratch install dir");
    let binary_name = "growth-layer-agent-bench";

    // A plausible-sized stand-in for a real binary -- staging is a copy
    // plus two renames, so its cost scales with artifact size, not with
    // being a real executable.
    let active_bytes = fill_bytes(5_000_000);
    std::fs::write(scratch.join(binary_name), &active_bytes).expect("write fake active binary");

    let new_binary_path = scratch.join("new-binary");
    std::fs::write(&new_binary_path, fill_bytes(5_200_000)).expect("write fake new binary");

    let staging = Staging::new(&scratch, binary_name);
    staging
        .stage_and_swap(&new_binary_path)
        .expect("stage_and_swap must succeed against this benchmark's own real files");
    staging
        .commit()
        .expect("commit must succeed immediately after a successful stage_and_swap");

    println!("staged, swapped, and committed a new binary at {}", scratch.display());
    let _ = std::fs::remove_dir_all(&scratch);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("usage: update_bench <download|apply> [...]");
    match mode.as_str() {
        "download" => {
            let artifact_bytes: usize = args
                .next()
                .unwrap_or_else(|| "20000000".to_string())
                .parse()
                .expect("artifact_bytes must be an integer");
            let max_bytes_per_second: Option<u64> = args.next().and_then(|s| s.parse().ok());
            run_download(artifact_bytes, max_bytes_per_second);
        }
        "apply" => run_apply(),
        other => panic!("unknown mode '{other}' -- expected 'download' or 'apply'"),
    }
}
