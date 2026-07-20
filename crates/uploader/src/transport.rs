use std::time::Duration;

/// What happened when a single record was POSTed. Deliberately NOT a raw
/// `Result<_, ureq::Error>` — that type's `Debug`/`Display` can embed the
/// request URL and response body, and this crate's "no raw payload/token in
/// logs" acceptance criterion means every layer above the transport must
/// only ever see a small, safe-to-log classification, never the underlying
/// HTTP error object.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportOutcome {
    Success {
        status: u16,
    },
    /// 429 — `retry_after` is `Some` when the server sent a (simple,
    /// integer-seconds) `Retry-After` header; honored in preference to our
    /// own computed backoff when present, since the server knows its own
    /// recovery time better than a guess would.
    RateLimited {
        retry_after: Option<Duration>,
    },
    /// 5xx — transient, worth backing off and retrying later.
    ServerError {
        status: u16,
    },
    /// 401 — the token itself is invalid/revoked (e.g. a newer device
    /// confirmed pairing and revoked this one, per
    /// `backend/app/routes/pairing.py`'s single-active-session model).
    /// Retrying with the SAME token will never succeed — this is reported
    /// distinctly so the caller stops immediately instead of backing off
    /// and retrying forever against a fundamentally dead credential.
    Unauthorized,
    /// Any other 4xx — a problem with this specific record's content, not
    /// with the server or the token. Retrying the SAME bytes will not
    /// change the outcome.
    ClientError {
        status: u16,
    },
    /// Connection failed, timed out, DNS failure, etc. — no HTTP response
    /// was ever received; this is the "offline" case.
    NetworkError,
}

/// Abstracts the actual HTTP call so the uploader's retry/backoff state
/// machine can be tested with deterministic, injected failure sequences
/// (see `tests/uploader_tests.rs`) without needing a live server that can
/// be made to return a 429/5xx/network-error on demand.
pub trait UploadTransport {
    fn post_record(&self, body: &[u8]) -> TransportOutcome;
}

/// The real transport: POSTs to the backend's
/// `/v1/ingest/productivity-record` endpoint via `ureq` (a small, blocking
/// HTTP client — no async runtime, consistent with this agent's minimal
/// resource-budget goal from ADR 0013).
pub struct UreqTransport {
    agent: ureq::Agent,
    endpoint: String,
    agent_token: String,
}

impl UreqTransport {
    /// `backend_url` is the base origin (e.g. `http://localhost:8000`), NOT
    /// the full ingest path — this appends `/v1/ingest/productivity-record`
    /// itself, matching the Python MVP's own `agent/core/uploader.py`
    /// (`backend_url.rstrip("/") + "/v1/ingest/productivity-record"`)
    /// exactly. A real end-to-end run against the actual backend (AG-REL-003)
    /// found this crate previously posted to the bare `backend_url` with no
    /// path at all — every real upload attempt silently 404'd and backed
    /// off forever, a real functional regression from the Python MVP's own
    /// behavior that no unit test (all built on a mocked `UploadTransport`)
    /// could have caught, only a genuine live request against a real server.
    pub fn new(backend_url: String, agent_token: String, request_timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new().timeout(request_timeout).build();
        let endpoint = format!(
            "{}/v1/ingest/productivity-record",
            backend_url.trim_end_matches('/')
        );
        UreqTransport {
            agent,
            endpoint,
            agent_token,
        }
    }
}

impl UploadTransport for UreqTransport {
    fn post_record(&self, body: &[u8]) -> TransportOutcome {
        let result = self
            .agent
            .post(&self.endpoint)
            .set("X-Agent-Token", &self.agent_token)
            .set("Content-Type", "application/json")
            .send_bytes(body);

        match result {
            Ok(response) => TransportOutcome::Success {
                status: response.status(),
            },
            Err(ureq::Error::Status(status, response)) => classify_status(status, &response),
            // Deliberately not inspecting/logging this error's Debug output
            // here or anywhere above this function: `ureq::Transport`'s
            // Display can include the request URL, and the caller must
            // never see anything more than this coarse classification.
            Err(ureq::Error::Transport(_)) => TransportOutcome::NetworkError,
        }
    }
}

fn classify_status(status: u16, response: &ureq::Response) -> TransportOutcome {
    match status {
        401 => TransportOutcome::Unauthorized,
        429 => {
            let retry_after = response
                .header("Retry-After")
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            TransportOutcome::RateLimited { retry_after }
        }
        500..=599 => TransportOutcome::ServerError { status },
        _ => TransportOutcome::ClientError { status },
    }
}
