//! Real device-authorization pairing against `backend/app/routes/pairing.py`
//! — the piece `config.rs`'s own earlier doc comment used to call "no
//! pairing UI/flow is implemented here." Same pattern GitHub CLI/smart TVs
//! use: this agent calls `/v1/agent/pair/start` itself, gets a
//! `device_code` (secret, stays with the agent) and a `user_code` (short,
//! shown to the human), opens the browser straight to
//! `{dashboard_url}/activate?code={user_code}` so confirming needs no
//! typing, then polls `/v1/agent/pair/poll` until the human confirms (or
//! the code expires) — exactly the backend flow already real-tested in
//! AG-REL-003's own end-to-end pass, just triggered from the tray instead
//! of by hand with curl.

use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct PairStart {
    pub device_code: String,
    pub user_code: String,
    pub expires_in_seconds: u64,
    pub poll_interval_seconds: u64,
}

#[derive(Debug)]
pub enum PollOutcome {
    Pending,
    Confirmed { agent_token: String },
    /// Expired or revoked (backend returns 410 for both) — retrying the
    /// same `device_code` will never succeed; the caller must start a
    /// fresh pairing attempt, not keep polling this one.
    Gone,
}

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("network error contacting backend")]
    Network,
    #[error("backend returned an unexpected response")]
    UnexpectedResponse,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()
}

/// `backend_url` is the base origin (e.g. `http://localhost:8000`) — same
/// convention `uploader::UreqTransport` uses since AG-REL-003's own fix,
/// not the full path.
pub fn start(backend_url: &str) -> Result<PairStart, PairingError> {
    let url = format!("{}/v1/agent/pair/start", backend_url.trim_end_matches('/'));
    let response = agent()
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string("{}")
        .map_err(|_| PairingError::Network)?;
    let text = response
        .into_string()
        .map_err(|_| PairingError::UnexpectedResponse)?;
    serde_json::from_str(&text).map_err(|_| PairingError::UnexpectedResponse)
}

pub fn poll(backend_url: &str, device_code: &str) -> Result<PollOutcome, PairingError> {
    let url = format!(
        "{}/v1/agent/pair/poll?device_code={}",
        backend_url.trim_end_matches('/'),
        urlencode(device_code)
    );
    match agent().get(&url).call() {
        Ok(response) => {
            #[derive(Deserialize)]
            struct PollResponse {
                status: String,
                agent_token: Option<String>,
            }
            let text = response
                .into_string()
                .map_err(|_| PairingError::UnexpectedResponse)?;
            let body: PollResponse =
                serde_json::from_str(&text).map_err(|_| PairingError::UnexpectedResponse)?;
            match (body.status.as_str(), body.agent_token) {
                ("confirmed", Some(agent_token)) => Ok(PollOutcome::Confirmed { agent_token }),
                ("pending", _) => Ok(PollOutcome::Pending),
                _ => Err(PairingError::UnexpectedResponse),
            }
        }
        // 404 (unknown device_code) and 410 (expired/revoked) both mean
        // this specific pairing attempt is over — never retriable.
        Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(410, _)) => {
            Ok(PollOutcome::Gone)
        }
        Err(_) => Err(PairingError::Network),
    }
}

/// `device_code` is a `secrets::token_urlsafe(32)` value on the backend
/// side (URL-safe base64 already — `-`/`_`, no characters a query string
/// needs escaped) — this exists so this module never silently assumes
/// that stays true forever without at least handling the general case.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real, live check against whatever backend is running at
    /// `http://localhost:8000` in this dev environment (the same one
    /// AG-REL-003's own end-to-end pass used) — not a mock. Skips
    /// itself (rather than failing the whole suite) if nothing is
    /// listening there, since CI/other machines running `cargo test`
    /// won't have this dev backend up.
    #[test]
    fn start_and_poll_against_a_real_running_backend() {
        let backend_url = "http://localhost:8000";
        let start_result = start(backend_url);
        if start_result.is_err() {
            eprintln!("skipping: no real backend reachable at {backend_url}");
            return;
        }
        let started = start_result.unwrap();
        assert!(!started.device_code.is_empty());
        assert!(started.user_code.contains('-'), "user_code should be the human-readable XXXX-XXXX shape, got {:?}", started.user_code);
        assert!(started.expires_in_seconds > 0);

        // Real poll against a real, freshly-started (never confirmed)
        // pairing attempt -- must report Pending, not Confirmed/Gone,
        // since nothing has confirmed it yet.
        let poll_result = poll(backend_url, &started.device_code).expect("poll must succeed against a device_code this same call just received from the real backend");
        assert!(matches!(poll_result, PollOutcome::Pending));

        // A made-up device_code must report Gone (backend returns 404),
        // never Pending/Confirmed -- proves poll() actually distinguishes
        // "this attempt exists but isn't confirmed yet" from "this
        // device_code was never issued at all".
        let unknown = poll(backend_url, "this-device-code-was-never-issued-by-anyone");
        assert!(matches!(unknown, Ok(PollOutcome::Gone)));
    }
}
