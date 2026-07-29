// Real, found-by-a-real-user bug: without this, Rust's default Windows
// build target is the "console" subsystem, so Windows allocates a real
// console window for every launch (Start Menu shortcut, autostart,
// double-click) -- an empty, do-nothing terminal that stays open for
// the process's entire lifetime (this agent never prints to stdout),
// exactly the "висит пустой терминал" symptom reported. `"windows"`
// subsystem means no console is ever created; the tray-only UI this
// agent already has (ADR 0013) was always the intended interface.
// `cfg_attr(windows, ...)` since this attribute doesn't exist as a
// concept on Linux/macOS (which never had a console-window problem to
// begin with) -- unconditional use would be a compile error there.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! `growth-layer-agent` — the single running process AG-WIN-002/
//! AG-LNX-003's installers package. Wires together every building-block
//! crate from AG-003 through AG-LNX-002 into one real agent: lifecycle
//! guarantees (single instance, crash detection, crash-restart
//! registration, autostart, rotating log, real session/power-event
//! registration), a platform signal collector (`windows_collector` or
//! `linux_collector`, chosen at compile time — see `platform.rs`),
//! `normalization`'s bucket accumulation, `durable-queue`'s crash-safe
//! local persistence, `uploader`'s resilient batch upload, and
//! `ui-shell`'s tray.
//!
//! Deliberately NOT a full re-implementation of `agent/main.py`'s
//! business logic (no git-commit scanning) — see config.rs's doc
//! comment for exactly where the line is drawn. Category overrides ARE
//! real here (`run_category_overrides_loop` below), but per-user and
//! backend-synced, not `agent/main.py`'s local-file-only
//! `config.yaml`'s `category_overrides` — a real user's Telegram
//! Desktop report is what prompted building a real one instead of
//! porting the old file-based mechanism. Device pairing (`pairing.rs`) WAS
//! initially out of scope but is now real (AG-REL-003 follow-up): the
//! tray's "Pair device" action calls the real backend pairing API
//! itself, the same flow previously only exercisable by hand with curl.
//! What IS real here: every wired crate runs its actual,
//! already-independently-reviewed code, not a stand-in.

mod config;
mod pairing;
mod paths;
mod platform;
mod update_check;

use chrono::Utc;
use collector_core::SignalCollector;
use durable_queue::{DurableQueue, QueueConfig};
use event_contract::{Consent, DeviceId, Envelope, NewEnvelope, Payload};
use lifecycle::{
    acquire, register_for_crash_restart, Autostart, CrashMarker, LifecycleAction, LifecycleState,
    RotatingLog,
};
use normalization::{BucketAccumulator, Tick, TitleRules, UrlRules};
use platform::{new_collector, NativeLoop};
use secrets::SecretString;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ui_shell::{run_tray, AgentController, AgentStatus};
use uploader::{BackoffConfig, BackoffState, Uploader, UploaderConfig, UreqTransport};

// Was a hand-typed literal that drifted from Cargo.toml's actual version
// (shipped v0.1.17 still displaying "0.1.16" in the tray, caught via a real
// user's screenshot after reinstalling) — derived from CARGO_PKG_VERSION at
// compile time now, so a version bump can never leave this one stale again.
const AGENT_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "-rust-prototype");
const AUTOSTART_APP_NAME: &str = "GrowthLayerAgent";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const EXPORT_INTERVAL_SECONDS: f64 = 60.0; // matches agent/config.yaml's override, not the 300s dataclass default
const UNEXPLAINED_GAP_THRESHOLD_SECONDS: f64 = 900.0;
const UPLOAD_INTERVAL: Duration = Duration::from_secs(30);
// Not instant on purpose — the dashboard's own confirmation message says
// so explicitly (TodayPage.tsx's category-override snackbar). A manual
// override is a rare, deliberate action, not something that needs
// sub-minute propagation; 5 minutes balances "notices fairly soon"
// against "don't hammer the backend from every idle agent".
const CATEGORY_OVERRIDES_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
// Same cadence/reasoning as CATEGORY_OVERRIDES_POLL_INTERVAL -- a
// person adding a browser-tab keyword rule right now cares about it
// taking effect soon, not instantly.
const TITLE_RULES_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
// Same cadence/reasoning as TITLE_RULES_POLL_INTERVAL -- a person adding a
// domain rule right now cares about it taking effect soon, not instantly.
const URL_RULES_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
// New versions don't ship often enough to justify a 5-minute cadence
// (that's `category_overrides`' rhythm, matched to a person actively
// fixing a miscategorized app right now) -- reaching everyone within a
// day of a real release is plenty, at a small fraction of the request
// volume.
const UPDATE_CHECK_POLL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Two independent reasons work can be paused, matching
/// `lifecycle::power_events::LifecycleState`'s own "suspended vs locked
/// as independent flags" reasoning exactly: a user pausing from the tray
/// and the OS suspending/locking must not clobber each other. Effective
/// pause is the OR of both — see `is_paused()`.
struct SharedState {
    user_paused: AtomicBool,
    system_paused: AtomicBool,
    pending_count: AtomicUsize,
    last_sync: Mutex<Option<chrono::DateTime<Utc>>>,
    paired: AtomicBool,
    dashboard_url: String,
    backend_url: String,
    /// Shared, mutable so a real pairing flow (`pairing.rs`) completing
    /// AFTER startup takes effect on `run_uploader_loop`'s very next
    /// cycle, without needing a restart — the loop reads this fresh each
    /// time instead of capturing one fixed value at spawn.
    agent_token: Mutex<SecretString>,
    /// Written by `run_category_overrides_loop`, read by
    /// `run_collector_loop` once per export cycle (see
    /// `set_category_overrides`'s doc comment on why not more often).
    /// Real per-user overrides from the dashboard — see this file's
    /// module doc comment for how this differs from `agent/main.py`'s
    /// old local-file-only mechanism.
    category_overrides: Mutex<BTreeMap<String, String>>,
    /// Written by `run_title_rules_loop`, read by `run_collector_loop`
    /// once per export cycle (same rhythm as `category_overrides`
    /// above) -- a user's own browser-tab keyword rules, e.g. "youtube"
    /// -> a custom "Отдых" category. Windows-only for now (see
    /// `run_collector_loop`'s `#[cfg(windows)]` consumption site) --
    /// the polling itself is cheap and harmless to run on every
    /// platform, only the collector-side application is gated.
    title_rules: Mutex<TitleRules>,
    /// Same idea as `title_rules` above, for GET /v1/agent/url-rules -- a
    /// user's own domain rules (e.g. "youtube" -> a custom "Отдых"
    /// category), matched against the address bar instead of the title.
    /// See `windows_collector::browser_url`'s doc comment for how the
    /// address bar is actually read -- Windows-only for now, same
    /// per-platform gating reasoning as `title_rules`.
    url_rules: Mutex<UrlRules>,
    /// Written by `run_update_check_loop`, read by the tray to decide
    /// what "Проверить обновления" shows/does — `None` means either "no
    /// update found yet" or "haven't checked yet," the tray doesn't need
    /// to tell those apart (both render the same "Проверить обновления"
    /// prompt).
    available_update: Mutex<Option<update_check::AvailableUpdate>>,
}

impl SharedState {
    fn is_paused(&self) -> bool {
        self.user_paused.load(Ordering::Relaxed) || self.system_paused.load(Ordering::Relaxed)
    }
}

struct Controller {
    state: Arc<SharedState>,
    log: Arc<RotatingLog>,
    device_id: String,
}

impl AgentController for Controller {
    fn status(&self) -> AgentStatus {
        AgentStatus {
            paired: self.state.paired.load(Ordering::Relaxed),
            is_paused: self.state.is_paused(),
            last_sync: *self.state.last_sync.lock().unwrap(),
            pending_count: self.state.pending_count.load(Ordering::Relaxed),
            agent_version: AGENT_VERSION.to_string(),
            available_update_version: self
                .state
                .available_update
                .lock()
                .unwrap()
                .as_ref()
                .map(|u| u.version.to_string()),
        }
    }

    fn toggle_active(&self) {
        self.state.user_paused.fetch_xor(true, Ordering::Relaxed);
    }

    fn dashboard_url(&self) -> String {
        // Was the bare root — a real user's report caught "Открыть дашборд"
        // landing on the marketing homepage instead of the actual dashboard.
        format!("{}/today", self.state.dashboard_url.trim_end_matches('/'))
    }

    fn diagnostics_url(&self) -> String {
        format!("{}/history", self.state.dashboard_url)
    }

    fn help_url(&self) -> String {
        "https://github.com/ak-alz/gla-client".to_string()
    }

    fn pair_device(&self) {
        if self.state.paired.load(Ordering::Relaxed) {
            return; // already paired -- "Pair device" isn't even shown then, but no-op if it somehow fires
        }
        let state = Arc::clone(&self.state);
        let log = Arc::clone(&self.log);
        std::thread::spawn(move || run_pairing_flow(state, log));
    }

    /// A known update already found by the background loop: no need to
    /// make the person wait on a network call they don't need right
    /// now -- open the release notes straight away. Nothing known yet:
    /// this IS the explicit, deliberate user action a background poll
    /// isn't, so a real (spawned, non-blocking-the-tray) check runs
    /// immediately rather than waiting up to `UPDATE_CHECK_POLL_INTERVAL`.
    fn check_for_updates(&self) {
        if let Some(update) = self.state.available_update.lock().unwrap().clone() {
            let _ = ui_shell::open_url(&update.release_notes_url);
            return;
        }
        let state = Arc::clone(&self.state);
        let device_id = self.device_id.clone();
        std::thread::spawn(move || {
            let installed_version =
                semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is always valid semver");
            if let Some(found) = update_check::check_once_live(&installed_version, &device_id) {
                *state.available_update.lock().unwrap() = Some(found);
            }
        });
    }
}

/// Real device-authorization pairing, triggered from the tray
/// (`Controller::pair_device`) — calls `/v1/agent/pair/start` itself,
/// opens the browser straight to the confirmation page with the
/// `user_code` prefilled (`ActivatePage.tsx`'s own `?code=` handling),
/// then polls until the human confirms or the code expires. Runs on its
/// own thread (spawned by the caller) since it blocks on network I/O and
/// sleeps between polls — must never run on the tray's own event-loop
/// thread. The code is also written to `agent.log` — the fallback for
/// "the browser didn't open" or "I closed the tab," since the tray has
/// no window of its own to display it in (ADR 0013).
fn run_pairing_flow(state: Arc<SharedState>, log: Arc<RotatingLog>) {
    let start = match pairing::start(&state.backend_url) {
        Ok(start) => start,
        Err(_) => {
            let _ = log.append("pairing failed to start (backend unreachable?) -- try again from the tray menu");
            return;
        }
    };
    let _ = log.append(&format!(
        "pairing code: {} (valid {} minutes) -- opening browser to confirm; if it didn't open, go to the dashboard's Device page and enter this code",
        start.user_code,
        start.expires_in_seconds / 60
    ));

    let activate_url = format!(
        "{}/activate?code={}",
        state.dashboard_url.trim_end_matches('/'),
        start.user_code
    );
    let _ = ui_shell::open_url(&activate_url);

    let deadline = std::time::Instant::now() + Duration::from_secs(start.expires_in_seconds);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(start.poll_interval_seconds));
        match pairing::poll(&state.backend_url, &start.device_code) {
            Ok(pairing::PollOutcome::Confirmed { agent_token }) => {
                let token = SecretString::new(agent_token);
                let _ = config::persist_agent_token(&token);
                *state.agent_token.lock().unwrap() = token;
                state.paired.store(true, Ordering::SeqCst);
                let _ = log.append("pairing confirmed");
                return;
            }
            Ok(pairing::PollOutcome::Pending) => continue,
            Ok(pairing::PollOutcome::Gone) | Err(_) => {
                let _ = log.append("pairing code expired or was never confirmed -- try again from the tray menu");
                return;
            }
        }
    }
    let _ = log.append("pairing code expired without confirmation");
}

fn autostart_handle() -> Autostart {
    let exe = std::env::current_exe().expect("current_exe must resolve");
    Autostart::new(AUTOSTART_APP_NAME, exe)
}

/// Installer post-install step (`[Run]` in agent.iss) — reuses the
/// already-reviewed `lifecycle::Autostart` rather than duplicating
/// registry-writing logic in Inno Setup's own scripting language.
fn register_autostart() {
    let _ = autostart_handle().enable();
}

/// Installer `[UninstallRun]` step — same reasoning as `register_autostart`.
fn unregister_autostart() {
    let _ = autostart_handle().disable();
}

fn run_collector_loop(
    state: Arc<SharedState>,
    queue: Arc<DurableQueue>,
    device_id: DeviceId,
    stop: Arc<AtomicBool>,
    log: Arc<RotatingLog>,
) {
    let mut collector = new_collector();
    if collector.start().is_err() {
        return;
    }

    // Реальный баг, найденный на живых машинах: на Ubuntu 24 (GNOME 46)
    // вся активность молча уходила в категорию "Другое", а на другой
    // машине (более новый GNOME) — нет, и до сих пор не было ни единой
    // строчки в логе, почему. `unsupported_reason()`/
    // `last_gnome_extension_error()` уже существовали в linux-collector,
    // но их никогда никто не вызывал из реально запускаемого бинарника
    // — это первое место, где это делается.
    #[cfg(target_os = "linux")]
    {
        if let Some(reason) = collector.unsupported_reason() {
            let detail = collector
                .last_gnome_extension_error()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default();
            let _ = log.append(&format!(
                "active-window detection unavailable at startup: {reason:?}{detail}"
            ));
        }
    }

    let consent = Consent {
        active_app_category: true,
        input_activity_counts: true,
        idle_tracking: true,
        activity_segments: true,
        unexplained_gaps: true,
        git_activity: false,
        app_detail: true,
    };
    let mut accumulator = BucketAccumulator::new(
        consent.clone(),
        BTreeMap::new(),
        UNEXPLAINED_GAP_THRESHOLD_SECONDS,
    );
    let mut bucket_started_at = Utc::now();
    #[cfg(target_os = "linux")]
    let mut was_unsupported = collector.unsupported_reason().is_some();

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(POLL_INTERVAL);
        let now = Utc::now();

        if !state.is_paused() {
            let snapshot = collector.poll();

            // `poll()` silently retries the GNOME extension probe every
            // tick while unsupported (see collector.rs) — log only the
            // actual transition, not every tick, so a real recovery
            // (extension finishes loading after login) or a real
            // regression (extension crashes/gets disabled mid-session)
            // is visible in agent.log without spamming it every 2s.
            #[cfg(target_os = "linux")]
            {
                let is_unsupported = collector.unsupported_reason().is_some();
                if is_unsupported != was_unsupported {
                    if is_unsupported {
                        let reason = collector.unsupported_reason();
                        let detail = collector
                            .last_gnome_extension_error()
                            .map(|e| format!(" ({e})"))
                            .unwrap_or_default();
                        let _ = log.append(&format!(
                            "active-window detection stopped working: {reason:?}{detail}"
                        ));
                    } else {
                        let _ = log.append("active-window detection started working");
                    }
                    was_unsupported = is_unsupported;
                }
            }

            let tick = Tick {
                active_process_name: snapshot.active_process_name,
                keyboard_events: snapshot.keyboard_events,
                mouse_move_events: snapshot.mouse_move_events,
                mouse_click_events: snapshot.mouse_click_events,
                is_idle: snapshot.is_idle,
                category_override: snapshot.category_override,
                occurred_at: now,
                interval_seconds: POLL_INTERVAL.as_secs_f64(),
            };
            accumulator.accumulate(&tick);
        }

        let bucket_age = (now - bucket_started_at).num_milliseconds() as f64 / 1000.0;
        if bucket_age >= EXPORT_INTERVAL_SECONDS {
            // Picked up once per export cycle, not every 2s tick — cheap
            // either way (a handful of strings at most), but this cadence
            // is the natural checkpoint and matches the dashboard's own
            // "applies at the next sync, not instantly" message. Only
            // affects ticks accumulated AFTER this point, never retroactive.
            accumulator.set_category_overrides(state.category_overrides.lock().unwrap().clone());
            // Windows-only, same as should_classify()'s whole premise
            // (browser_title.rs) -- linux-collector/macos-collector don't
            // examine window titles at all (no title reading exists on
            // either), so there's no setter to call there.
            #[cfg(windows)]
            collector.set_browser_title_rules(state.title_rules.lock().unwrap().clone());
            // URL rules, unlike title rules, ARE wired on all three
            // platforms -- Windows (UIA, empirically verified against a
            // live browser), Linux (AT-SPI2) and macOS (Accessibility
            // API) both gained a real `browser_url` module in the same
            // round that added this. See each crate's `browser_url.rs`
            // doc comment for its own verification status -- only
            // Windows' has actually been run against a real browser.
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            collector.set_browser_url_rules(state.url_rules.lock().unwrap().clone());
            let signals = accumulator.flush(None); // git_commits_count: out of scope, see config.rs
            match Envelope::build_or_quarantine(NewEnvelope {
                device_id,
                agent_version: AGENT_VERSION.to_string(),
                payload: Payload {
                    period_start: bucket_started_at,
                    period_end: now,
                    signals,
                    consent: consent.clone(),
                    signature: None,
                },
            }) {
                Ok(envelope) => {
                    let _ = queue.enqueue(&envelope);
                }
                Err(quarantined) => {
                    // No on-disk quarantine plumbing exists for a contract
                    // violation raised outside DurableQueue itself (its
                    // quarantine/ subdir is for corrupt-on-disk records,
                    // a different failure mode) — this whole bucket's
                    // signals are dropped, not persisted anywhere. An
                    // independent review found this is NOT purely
                    // theoretical: a backward system-clock adjustment
                    // (NTP correction, sleep/resume clock skew) between
                    // `bucket_started_at` and `now` makes
                    // `period_end < period_start`, which is exactly a
                    // `ContractViolation` this crate already checks for.
                    // Still narrow and self-healing (the next bucket
                    // starts fresh), but a silent loss deserves at least
                    // a trace instead of vanishing with zero record.
                    let _ = log.append(&format!(
                        "bucket dropped: envelope failed validation: {}",
                        quarantined.violations.join("; ")
                    ));
                }
            }
            state
                .pending_count
                .store(queue.pending_count().unwrap_or(0), Ordering::Relaxed);
            bucket_started_at = now;
        }
    }

    collector.stop();
}

fn run_uploader_loop(
    state: Arc<SharedState>,
    queue: Arc<DurableQueue>,
    backend_url: String,
    stop: Arc<AtomicBool>,
) {
    let mut backoff_state = BackoffState::new();

    while !stop.load(Ordering::Relaxed) {
        // Rebuilt every cycle (cheap -- no I/O until a request is
        // actually sent) so a token obtained via a real-time pairing flow
        // (`pairing.rs`) takes effect on the very next cycle, instead of
        // needing a restart. The one, deliberate, visible-in-a-diff call
        // site where the token leaves its `SecretString` wrapper —
        // `UreqTransport` needs a plain `String` to build its request
        // header (`transport.rs`'s own doc comment already documents why
        // it never logs that value further).
        let agent_token = state.agent_token.lock().unwrap().clone();
        let transport = UreqTransport::new(
            backend_url.clone(),
            agent_token.expose().to_string(),
            Duration::from_secs(10),
        );
        let uploader = Uploader::new(
            &transport,
            UploaderConfig {
                batch_size: 20,
                backoff: BackoffConfig::default(),
            },
        );

        let outcome = uploader.run_once(&queue, &mut backoff_state);
        state
            .pending_count
            .store(queue.pending_count().unwrap_or(0), Ordering::Relaxed);

        let sleep_for = match outcome {
            uploader::CycleOutcome::Idle => UPLOAD_INTERVAL,
            uploader::CycleOutcome::Progress { .. } => {
                *state.last_sync.lock().unwrap() = Some(Utc::now());
                Duration::from_secs(1) // more may be pending — retry soon
            }
            uploader::CycleOutcome::Backoff { after, .. } => after,
            uploader::CycleOutcome::Unauthorized => UPLOAD_INTERVAL, // token needs reconfiguring; no point retrying faster
        };
        sleep_in_slices(sleep_for, &stop);
    }
}

/// Periodically pulls GET /v1/agent/category-overrides and writes the
/// result into `state.category_overrides` — the first-ever agent←backend
/// config channel this agent has (see this file's module doc comment).
/// Prompted by a real user's report: "Telegram Desktop" isn't in the
/// built-in category map and there was no way to fix that without
/// hand-editing a local config file and restarting the agent.
///
/// Every successful fetch is cached to disk
/// (`paths::category_overrides_cache_path()`) so a restart while
/// offline still has the last known overrides instead of silently
/// reverting to none — same "don't lose state just because the network
/// is briefly down" spirit as `durable-queue`. The cache is read once,
/// at startup, before the first fetch completes; a failed fetch after
/// that just keeps whatever is already in `state.category_overrides`.
fn run_category_overrides_loop(state: Arc<SharedState>, backend_url: String, stop: Arc<AtomicBool>) {
    if let Some(cached) = load_cached_category_overrides() {
        *state.category_overrides.lock().unwrap() = cached;
    }

    while !stop.load(Ordering::Relaxed) {
        let agent_token = state.agent_token.lock().unwrap().clone();
        let url = format!("{}/v1/agent/category-overrides", backend_url.trim_end_matches('/'));
        let fetched = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build()
            .get(&url)
            .set("X-Agent-Token", agent_token.expose())
            .call()
            .ok()
            .and_then(|resp| resp.into_string().ok())
            .and_then(|text| serde_json::from_str::<BTreeMap<String, String>>(&text).ok());

        if let Some(overrides) = fetched {
            let _ = std::fs::write(
                paths::category_overrides_cache_path(),
                serde_json::to_vec(&overrides).unwrap_or_default(),
            );
            *state.category_overrides.lock().unwrap() = overrides;
        }
        // Network/auth failures are silently retried next cycle -- same
        // "not worth a dedicated error path" reasoning as
        // run_uploader_loop's Unauthorized arm; a stale but present
        // override beats none, and a bad agent_token is already
        // surfaced elsewhere (the tray's connection status).

        sleep_in_slices(CATEGORY_OVERRIDES_POLL_INTERVAL, &stop);
    }
}

fn load_cached_category_overrides() -> Option<BTreeMap<String, String>> {
    let bytes = std::fs::read(paths::category_overrides_cache_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Same shape as `run_category_overrides_loop` above, for GET
/// /v1/agent/title-rules -- a user's own browser-tab keyword rules
/// (real feedback: "if my active tab is YouTube, I want to know how
/// much I watch it, and bucket that into my own 'Отдых' category").
/// The response is already `[[category, [keyword, ...]]]` -- exactly
/// `TitleRules`'s own shape, no client-side grouping needed.
fn run_title_rules_loop(state: Arc<SharedState>, backend_url: String, stop: Arc<AtomicBool>) {
    if let Some(cached) = load_cached_title_rules() {
        *state.title_rules.lock().unwrap() = cached;
    }

    while !stop.load(Ordering::Relaxed) {
        let agent_token = state.agent_token.lock().unwrap().clone();
        let url = format!("{}/v1/agent/title-rules", backend_url.trim_end_matches('/'));
        let fetched = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build()
            .get(&url)
            .set("X-Agent-Token", agent_token.expose())
            .call()
            .ok()
            .and_then(|resp| resp.into_string().ok())
            .and_then(|text| serde_json::from_str::<TitleRules>(&text).ok());

        if let Some(rules) = fetched {
            let _ = std::fs::write(paths::title_rules_cache_path(), serde_json::to_vec(&rules).unwrap_or_default());
            *state.title_rules.lock().unwrap() = rules;
        }
        // Same "stale but present beats none" reasoning as
        // run_category_overrides_loop -- a transient network/auth
        // failure keeps whatever rules were already known.

        sleep_in_slices(TITLE_RULES_POLL_INTERVAL, &stop);
    }
}

fn load_cached_title_rules() -> Option<TitleRules> {
    let bytes = std::fs::read(paths::title_rules_cache_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Same shape as `run_title_rules_loop` above, for GET /v1/agent/url-rules
/// -- a user's own domain rules, matched against the address bar (see
/// `windows_collector::browser_url`'s doc comment for how that's actually
/// read). Same `[[category, [keyword, ...]]]` response shape as title
/// rules -- `UrlRules` and `TitleRules` are structurally identical, just
/// configured independently (see `normalization::url_classifier`'s doc
/// comment on why they're still distinct types).
fn run_url_rules_loop(state: Arc<SharedState>, backend_url: String, stop: Arc<AtomicBool>) {
    if let Some(cached) = load_cached_url_rules() {
        *state.url_rules.lock().unwrap() = cached;
    }

    while !stop.load(Ordering::Relaxed) {
        let agent_token = state.agent_token.lock().unwrap().clone();
        let url = format!("{}/v1/agent/url-rules", backend_url.trim_end_matches('/'));
        let fetched = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build()
            .get(&url)
            .set("X-Agent-Token", agent_token.expose())
            .call()
            .ok()
            .and_then(|resp| resp.into_string().ok())
            .and_then(|text| serde_json::from_str::<UrlRules>(&text).ok());

        if let Some(rules) = fetched {
            let _ = std::fs::write(paths::url_rules_cache_path(), serde_json::to_vec(&rules).unwrap_or_default());
            *state.url_rules.lock().unwrap() = rules;
        }
        // Same "stale but present beats none" reasoning as
        // run_title_rules_loop -- a transient network/auth failure keeps
        // whatever rules were already known.

        sleep_in_slices(URL_RULES_POLL_INTERVAL, &stop);
    }
}

fn load_cached_url_rules() -> Option<UrlRules> {
    let bytes = std::fs::read(paths::url_rules_cache_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Real periodic check for `update_check.rs` — see that module's doc
/// comment for the full story on why this loop didn't exist until now
/// despite the crates it wires up already being built and tested.
/// `installed_version` is deliberately `CARGO_PKG_VERSION` alone, never
/// `AGENT_VERSION` (see `update_check.rs`'s doc comment on why the
/// "-rust-prototype" display suffix must never enter a semver compare).
fn run_update_check_loop(state: Arc<SharedState>, device_id: String, stop: Arc<AtomicBool>) {
    let installed_version =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is always valid semver");

    while !stop.load(Ordering::Relaxed) {
        let found = update_check::check_once_live(&installed_version, &device_id);
        if found.is_some() {
            *state.available_update.lock().unwrap() = found;
        }
        // A failed/absent check leaves whatever was already known in
        // place -- same "stale-but-present beats none" reasoning as
        // run_category_overrides_loop, and a real update never
        // disappears from the manifest once published, so there's no
        // "un-find" case to handle here.
        sleep_in_slices(UPDATE_CHECK_POLL_INTERVAL, &stop);
    }
}

fn run_power_loop(state: Arc<SharedState>, stop: Arc<AtomicBool>) {
    let Ok((mut native_loop, rx)) = NativeLoop::start() else {
        return;
    };
    let mut lifecycle_state = LifecycleState::new();

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => match lifecycle_state.handle(event) {
                LifecycleAction::PauseWork => state.system_paused.store(true, Ordering::Relaxed),
                LifecycleAction::ResumeWork => state.system_paused.store(false, Ordering::Relaxed),
                LifecycleAction::PrepareToExit | LifecycleAction::Continue => {}
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    native_loop.stop();
}

/// Sleeps `total`, but in short slices so `stop` being set doesn't force
/// waiting out the full backoff/upload interval before the process can
/// exit promptly on Quit.
fn sleep_in_slices(total: Duration, stop: &AtomicBool) {
    const SLICE: Duration = Duration::from_millis(200);
    let mut remaining = total;
    while remaining > Duration::ZERO && !stop.load(Ordering::Relaxed) {
        let this_slice = remaining.min(SLICE);
        std::thread::sleep(this_slice);
        remaining = remaining.saturating_sub(this_slice);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--register-autostart") {
        register_autostart();
        return;
    }
    if args.iter().any(|a| a == "--unregister-autostart") {
        unregister_autostart();
        return;
    }

    std::fs::create_dir_all(paths::data_dir()).expect("create data dir");
    std::fs::create_dir_all(paths::log_dir()).expect("create log dir");
    // "Local DB permissions" (AG-SEC-001) — the data dir holds
    // `device_id.json`/`config.json`/the queue, none of which any
    // other local account should be able to read. Best-effort: a
    // pre-existing dir from before this hardening existed keeps
    // whatever permissions it already had if this call fails, rather
    // than blocking startup over it.
    let _ = secrets::restrict_to_current_user_only(&paths::data_dir());

    let log =
        Arc::new(RotatingLog::new(paths::log_dir(), "agent.log", 1_000_000, 5).expect("open log"));

    let _instance_guard = match acquire(&paths::single_instance_lock_path()) {
        Ok(guard) => guard,
        Err(_) => {
            let _ = log.append("startup aborted: another instance is already running");
            return;
        }
    };

    let crash_marker = CrashMarker::new(paths::crash_marker_path());
    if crash_marker.previous_run_crashed() {
        let _ = log.append("previous run did not exit cleanly");
    }
    let _ = crash_marker.mark_running();
    let _ = register_for_crash_restart("--restarted-after-crash");

    let device_id = DeviceId::load_or_create(&paths::device_id_path()).expect("device id");
    let queue = Arc::new(
        DurableQueue::open(QueueConfig {
            dir: paths::queue_dir(),
            max_pending_bytes: 20 * 1024 * 1024,
            acked_retention: chrono::Duration::days(7),
        })
        .expect("open queue"),
    );
    let cfg = config::load();

    let state = Arc::new(SharedState {
        user_paused: AtomicBool::new(false),
        system_paused: AtomicBool::new(false),
        pending_count: AtomicUsize::new(queue.pending_count().unwrap_or(0)),
        last_sync: Mutex::new(None),
        paired: AtomicBool::new(!cfg.agent_token.is_empty()),
        dashboard_url: cfg.dashboard_url.clone(),
        backend_url: cfg.backend_url.clone(),
        agent_token: Mutex::new(cfg.agent_token.clone()),
        category_overrides: Mutex::new(BTreeMap::new()),
        title_rules: Mutex::new(Vec::new()),
        url_rules: Mutex::new(Vec::new()),
        available_update: Mutex::new(None),
    });

    let stop = Arc::new(AtomicBool::new(false));

    let device_id_string = device_id.to_string();
    let collector_thread = {
        let state = Arc::clone(&state);
        let queue = Arc::clone(&queue);
        let stop = Arc::clone(&stop);
        let log = Arc::clone(&log);
        std::thread::spawn(move || run_collector_loop(state, queue, device_id, stop, log))
    };
    let uploader_thread = {
        let state = Arc::clone(&state);
        let queue = Arc::clone(&queue);
        let stop = Arc::clone(&stop);
        let backend_url = cfg.backend_url.clone();
        std::thread::spawn(move || run_uploader_loop(state, queue, backend_url, stop))
    };
    let category_overrides_thread = {
        let state = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        let backend_url = cfg.backend_url.clone();
        std::thread::spawn(move || run_category_overrides_loop(state, backend_url, stop))
    };
    let title_rules_thread = {
        let state = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        let backend_url = cfg.backend_url.clone();
        std::thread::spawn(move || run_title_rules_loop(state, backend_url, stop))
    };
    let url_rules_thread = {
        let state = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        let backend_url = cfg.backend_url.clone();
        std::thread::spawn(move || run_url_rules_loop(state, backend_url, stop))
    };
    let power_thread = {
        let state = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || run_power_loop(state, stop))
    };
    let update_check_thread = {
        let state = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        let device_id_string = device_id_string.clone();
        std::thread::spawn(move || run_update_check_loop(state, device_id_string, stop))
    };

    // `systemctl --user stop`/`restart` sends SIGTERM by default (its
    // `KillSignal`) — without reacting to it, that ordinary service stop
    // is indistinguishable from a crash on the next startup (found by
    // AG-LNX-003's independent review). Converges onto the exact same
    // shutdown path `run_tray` already takes when the user clicks Quit,
    // rather than duplicating the stop/join/log/mark-clean-exit sequence
    // below for a second call site.
    #[cfg(target_os = "linux")]
    {
        use signal_hook::consts::{SIGINT, SIGTERM};
        use signal_hook::iterator::Signals;
        let mut signals = Signals::new([SIGTERM, SIGINT]).expect("register SIGTERM/SIGINT handler");
        std::thread::spawn(move || {
            if signals.forever().next().is_some() {
                ui_shell::request_quit();
            }
        });
    }

    let _ = log.append("agent started");
    let controller = Arc::new(Controller {
        state: Arc::clone(&state),
        log: Arc::clone(&log),
        device_id: device_id_string.clone(),
    });
    let _ = run_tray(controller);

    stop.store(true, Ordering::SeqCst);
    let _ = collector_thread.join();
    let _ = uploader_thread.join();
    let _ = power_thread.join();
    let _ = category_overrides_thread.join();
    let _ = title_rules_thread.join();
    let _ = url_rules_thread.join();
    let _ = update_check_thread.join();

    let _ = log.append("agent quit cleanly");
    let _ = crash_marker.mark_clean_exit();
}
