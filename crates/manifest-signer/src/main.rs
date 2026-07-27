//! Release-only tool: builds an `UnsignedManifest` from a real, already-
//! built and already-uploaded release artifact, signs it with the local
//! private key (never committed — see `~/.gla-manifest-signing-key` or
//! wherever `--key` points), and writes `manifest-<platform>-<arch>.json`
//! ready to attach as a GitHub Releases asset.
//!
//! Deliberately a plain hand-rolled arg parser, not `clap` — this is a
//! one-off local tool run by a human a few times a release, matching
//! the rest of this workspace's minimal-dependency ethos (no dependency
//! not already justified by `agent-bin` itself).

use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use update_manifest::{sign, Architecture, Channel, Platform, UnsignedManifest};

fn usage() -> ! {
    eprintln!(
        "usage: manifest-signer --version 0.1.21 --platform windows|linux|macos \\\n  \
         --artifact-url https://... --artifact-path ./GrowthLayerAgentSetup-0.1.21.exe \\\n  \
         --release-notes-url https://... [--rollout 100] [--mandatory] \\\n  \
         [--key ~/.gla-manifest-signing-key] [--out manifest-windows-x86_64.json] \\\n  \
         [--min-backend 0.1.0] [--min-schema 0.1.0] [--rollback-target 0.1.19]"
    );
    std::process::exit(2);
}

fn parse_args() -> HashMap<String, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut map = HashMap::new();
    let mut mandatory = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--mandatory" {
            mandatory = true;
            args.remove(i);
            continue;
        }
        if let Some(key) = args[i].strip_prefix("--") {
            let value = args.get(i + 1).unwrap_or_else(|| usage()).clone();
            map.insert(key.to_string(), value);
            i += 2;
        } else {
            usage();
        }
    }
    if mandatory {
        map.insert("mandatory".to_string(), "true".to_string());
    }
    map
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("signing key file must be valid hex"))
        .collect()
}

fn main() {
    let args = parse_args();
    let get = |k: &str| args.get(k).cloned().unwrap_or_else(|| usage());

    let version = semver::Version::parse(&get("version")).expect("--version must be valid semver");
    let platform = match get("platform").as_str() {
        "windows" => Platform::Windows,
        "linux" => Platform::Linux,
        "macos" => Platform::Macos,
        other => {
            eprintln!("unknown --platform {other:?}, expected windows|linux|macos");
            std::process::exit(2);
        }
    };
    let artifact_url = get("artifact-url");
    let artifact_path = PathBuf::from(get("artifact-path"));
    let release_notes_url = get("release-notes-url");
    let rollout_percentage: u8 = args
        .get("rollout")
        .map(|v| v.parse().expect("--rollout must be 0-100"))
        .unwrap_or(100);
    let mandatory = args.get("mandatory").is_some();
    let min_compatible_backend = semver::Version::parse(&args.get("min-backend").cloned().unwrap_or_else(|| "0.1.0".to_string()))
        .expect("--min-backend must be valid semver");
    let min_compatible_schema = semver::Version::parse(&args.get("min-schema").cloned().unwrap_or_else(|| "0.1.0".to_string()))
        .expect("--min-schema must be valid semver");
    let rollback_target = args
        .get("rollback-target")
        .map(|v| semver::Version::parse(v).expect("--rollback-target must be valid semver"));

    let key_path = args
        .get("key")
        .cloned()
        .unwrap_or_else(|| format!("{}/.gla-manifest-signing-key", std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).expect("no HOME/USERPROFILE")));
    let key_hex = std::fs::read_to_string(&key_path)
        .unwrap_or_else(|e| panic!("could not read signing key at {key_path}: {e}"));
    let key_bytes = from_hex(key_hex.trim());
    let signing_key = SigningKey::from_bytes(
        &key_bytes.try_into().expect("signing key file must be exactly 32 bytes (64 hex chars)"),
    );

    let artifact_bytes = std::fs::read(&artifact_path)
        .unwrap_or_else(|e| panic!("could not read artifact at {}: {e}", artifact_path.display()));
    let artifact_sha256 = to_hex(&Sha256::digest(&artifact_bytes));

    let manifest = UnsignedManifest {
        version: version.clone(),
        channel: Channel::Stable,
        platform,
        architecture: Architecture::X86_64,
        min_compatible_backend,
        min_compatible_schema,
        artifact_url,
        artifact_sha256,
        release_notes_url,
        rollout_percentage,
        mandatory,
        rollback_target,
    };

    let signed = sign(manifest, &signing_key);

    let out_path = args
        .get("out")
        .cloned()
        .unwrap_or_else(|| format!("manifest-{}-x86_64.json", get("platform")));
    std::fs::write(&out_path, serde_json::to_vec_pretty(&signed).unwrap()).unwrap();

    // Sanity self-check before declaring success — a manifest that
    // doesn't verify against its own embedded public key would be
    // silently useless to every agent that fetches it.
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    update_manifest::verify(&signed, &verifying_key).expect("just-signed manifest failed to self-verify");

    println!("wrote {out_path} (version {version}, public key {})", to_hex(&verifying_key.to_bytes()));
}
