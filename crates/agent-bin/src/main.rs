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
//! business logic (no `config.yaml`-equivalent consent/category-override
//! schema, no git-commit scanning) — see config.rs's doc comment for
//! exactly where the line is drawn. Device pairing (`pairing.rs`) WAS
//! initially out of scope but is now real (AG-REL-003 follow-up): the
//! tray's "Pair device" action calls the real backend pairing API
//! itself, the same flow previously only exercisable by hand with curl.
//! What IS real here: every wired crate runs its actual,
//! already-independently-reviewed code, not a stand-in.

mod config;
mod pairing;
mod paths;
mod platform;

use chrono::Utc;
use collector_core::SignalCollector;
use durable_queue::{DurableQueue, QueueConfig};
use event_contract::{Consent, DeviceId, Envelope, NewEnvelope, Payload};
use lifecycle::{
    acquire, register_for_crash_restart, Autostart, CrashMarker, LifecycleAction, LifecycleState,
    RotatingLog,
};
use normalization::{BucketAccumulator, Tick};
use platform::{new_collector, NativeLoop};
use secrets::SecretString;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ui_shell::{run_tray, AgentController, AgentStatus};
use uploader::{BackoffConfig, BackoffState, Uploader, UploaderConfig, UreqTransport};

const AGENT_VERSION: &str = "0.1.11-rust-prototype";
const AUTOSTART_APP_NAME: &str = "GrowthLayerAgent";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const EXPORT_INTERVAL_SECONDS: f64 = 60.0; // matches agent/config.yaml's override, not the 300s dataclass default
const UNEXPLAINED_GAP_THRESHOLD_SECONDS: f64 = 900.0;
const UPLOAD_INTERVAL: Duration = Duration::from_secs(30);

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
}

impl SharedState {
    fn is_paused(&self) -> bool {
        self.user_paused.load(Ordering::Relaxed) || self.system_paused.load(Ordering::Relaxed)
    }
}

struct Controller {
    state: Arc<SharedState>,
    log: Arc<RotatingLog>,
}

impl AgentController for Controller {
    fn status(&self) -> AgentStatus {
        AgentStatus {
            paired: self.state.paired.load(Ordering::Relaxed),
            is_paused: self.state.is_paused(),
            last_sync: *self.state.last_sync.lock().unwrap(),
            pending_count: self.state.pending_count.load(Ordering::Relaxed),
            agent_version: AGENT_VERSION.to_string(),
        }
    }

    fn toggle_active(&self) {
        self.state.user_paused.fetch_xor(true, Ordering::Relaxed);
    }

    fn dashboard_url(&self) -> String {
        self.state.dashboard_url.clone()
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

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(POLL_INTERVAL);
        let now = Utc::now();

        if !state.is_paused() {
            let snapshot = collector.poll();
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
    });

    let stop = Arc::new(AtomicBool::new(false));

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
    let power_thread = {
        let state = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || run_power_loop(state, stop))
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
    });
    let _ = run_tray(controller);

    stop.store(true, Ordering::SeqCst);
    let _ = collector_thread.join();
    let _ = uploader_thread.join();
    let _ = power_thread.join();

    let _ = log.append("agent quit cleanly");
    let _ = crash_marker.mark_clean_exit();
}
