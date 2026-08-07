//! `LinuxSignalCollector` — ties `environment`/`x11`/`hyprland`/
//! `evdev_counter`/`process_name` together into `collector_core::
//! SignalCollector`, mirroring `windows_collector::WindowsSignalCollector`'s
//! role exactly: `start()` detects the environment and starts whichever
//! backend applies, `poll()` returns one `RawSignalSnapshot`.

use crate::browser_url::{classify_browser_url, should_classify_via_url, AddressBarReader};
use crate::environment::{detect_active_window_backend, ActiveWindowBackend, UnsupportedReason};
use crate::evdev_counter::EvdevInputMonitor;
use crate::gnome_extension::{self, GnomeExtensionSession};
use crate::hyprland;
use crate::input_counters::InputCounters;
use crate::kwin_script::{self, KWinScriptSession};
use crate::process_name::process_name_for_pid;
use crate::x11::{X11Error, X11Session};
use collector_core::{RawSignalSnapshot, SignalCollector};
use normalization::UrlRules;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error(transparent)]
    X11(#[from] X11Error),
    #[error("no /dev/input devices could be opened for input-event counting (check `input` group membership)")]
    NoInputDevices,
}

enum ActiveWindowSource {
    // Boxed: `X11Session` (holding a full `RustConnection`) is ~500
    // bytes, dwarfing the other variants — clippy's `large_enum_variant`
    // correctly flags the unboxed form as wasting that much space in
    // every `ActiveWindowSource`, even the common non-X11 cases.
    X11(Box<X11Session>),
    Hyprland(PathBuf),
    GnomeExtension(GnomeExtensionSession),
    KWinScript(KWinScriptSession),
    Unsupported(UnsupportedReason),
}

pub struct LinuxSignalCollector {
    idle_threshold_seconds: f64,
    source: Option<ActiveWindowSource>,
    input_counters: Arc<InputCounters>,
    evdev_monitor: Option<EvdevInputMonitor>,
    browser_process_names: HashSet<String>,
    browser_url_rules: UrlRules,
    /// Company Layer — independent from `browser_url_rules` above, see
    /// `normalization::Tick::company_category`'s doc comment.
    company_browser_url_rules: UrlRules,
    /// Opt-in personal domain tally — see
    /// `windows_collector::WindowsSignalCollector`'s field of the same name.
    domain_tracking_enabled: bool,
    address_bar_reader: AddressBarReader,
    // Обычный `UnsupportedReason::GnomeRequiresShellExtension` (см. его
    // докстринг) намеренно не различает "не установлено"/"не включено"/
    // "ещё не подхватилось после логина"/что-то реально сломанное в самом
    // расширении — но сам `zbus::Error` под капотом эту разницу знает.
    // Раньше он терялся на `.ok()` в start()/poll() ниже — реальный баг
    // (Ubuntu 24 весь день падает в "Другое") оказалось физически нечем
    // диагностировать без единой строки в лог. Храним последний текст
    // ошибки отдельно, чтобы вызывающая сторона (agent-bin) могла его
    // реально куда-то записать.
    last_gnome_error: Option<String>,
    /// KDE-шный аналог поля выше, по той же причине: `KdeRequiresKWinScript`
    /// сам по себе не говорит, скрипт не установлен, выключен, или KWin
    /// вообще не отвечает — а текст ошибки говорит.
    last_kwin_error: Option<String>,
}

impl LinuxSignalCollector {
    pub fn new(idle_threshold_seconds: f64) -> Self {
        LinuxSignalCollector {
            idle_threshold_seconds,
            source: None,
            input_counters: Arc::new(InputCounters::new()),
            evdev_monitor: None,
            last_gnome_error: None,
            last_kwin_error: None,
            // Same six-browser list as windows_collector::platform's
            // browser_process_names() — Linux process names have no
            // ".exe" suffix, so they're spelled slightly differently, but
            // it's the same underlying six real browsers.
            browser_process_names: [
                "chrome", "google-chrome", "chromium", "firefox", "brave", "opera",
            ]
            .into_iter()
            .map(|name| name.to_lowercase())
            .collect(),
            browser_url_rules: Vec::new(),
            company_browser_url_rules: Vec::new(),
            domain_tracking_enabled: false,
            address_bar_reader: AddressBarReader::new(),
        }
    }

    /// Same idea as `windows_collector::WindowsSignalCollector::
    /// set_browser_url_rules` — refreshes URL-classification rules on an
    /// already-running collector.
    pub fn set_browser_url_rules(&mut self, browser_url_rules: UrlRules) {
        self.browser_url_rules = browser_url_rules;
    }

    /// Company Layer counterpart — see
    /// `windows_collector::WindowsSignalCollector::set_company_browser_url_rules`.
    pub fn set_company_browser_url_rules(&mut self, rules: UrlRules) {
        self.company_browser_url_rules = rules;
    }

    /// See `windows_collector::WindowsSignalCollector::set_domain_tracking_enabled`.
    pub fn set_domain_tracking_enabled(&mut self, enabled: bool) {
        self.domain_tracking_enabled = enabled;
    }

    /// The reason active-window detection is unavailable in the current
    /// session, if any — exposed so a caller (e.g. the tray's
    /// diagnostics view) can show a real, specific explanation rather
    /// than a bare "unavailable." Matches this task's own "Missing
    /// capability returns explicit status" acceptance criterion.
    pub fn unsupported_reason(&self) -> Option<&UnsupportedReason> {
        match &self.source {
            Some(ActiveWindowSource::Unsupported(reason)) => Some(reason),
            _ => None,
        }
    }

    /// The actual D-Bus error text from the most recent failed GNOME
    /// Shell extension probe, if the reason above is
    /// `GnomeRequiresShellExtension` — `None` for every other backend/
    /// reason, and `None` once the extension starts responding
    /// successfully (see `poll()`'s retry).
    pub fn last_gnome_extension_error(&self) -> Option<&str> {
        self.last_gnome_error.as_deref()
    }

    /// The KDE counterpart of the accessor above — why the companion KWin
    /// script isn't answering (not loaded, KWin not reachable, or the
    /// session-bus name already taken by another agent instance).
    pub fn last_kwin_script_error(&self) -> Option<&str> {
        self.last_kwin_error.as_deref()
    }
}

/// KDE Wayland: own the bus name, (re)install/enable/reload the script,
/// then ask KWin whether it's actually loaded. Factored out because
/// `start()` and `poll()`'s retry need this exact sequence, and the order
/// is load-bearing — the name must exist before the reload, or the
/// script's load-time push lands nowhere (see `kwin_script.rs`'s module
/// doc).
///
/// Unlike GNOME's probe, liveness cannot be "did a call succeed": the
/// script pushes, so an idle desktop legitimately has nothing to report.
/// `isScriptLoaded` is the only honest signal.
fn start_kwin_script() -> Result<KWinScriptSession, String> {
    let session = KWinScriptSession::start().map_err(|err| err.to_string())?;
    session.try_enable();
    match session.is_script_loaded() {
        Ok(true) => Ok(session),
        Ok(false) => Err(format!(
            "KWin reports the companion script (plugin id {}) is not loaded",
            kwin_script::PLUGIN_ID
        )),
        Err(err) => Err(err.to_string()),
    }
}

impl SignalCollector for LinuxSignalCollector {
    type Error = CollectorError;

    fn start(&mut self) -> Result<(), CollectorError> {
        let backend = detect_active_window_backend(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
            std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().as_deref(),
        );

        self.source = Some(match backend {
            ActiveWindowBackend::X11 => ActiveWindowSource::X11(Box::new(X11Session::connect()?)),
            ActiveWindowBackend::Hyprland => hyprland::socket_path(
                std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
                std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().as_deref(),
            )
            .map(ActiveWindowSource::Hyprland)
            .unwrap_or(ActiveWindowSource::Unsupported(
                UnsupportedReason::UnknownSessionType,
            )),
            // A `Connection::session()` succeeding only means a session
            // bus exists — it says nothing about whether the companion
            // Shell extension is actually loaded (not installed, not
            // enabled, or installed but awaiting the next login on
            // Wayland all look identical at the connection step). The
            // ONE real liveness check is an actual method call: if it
            // fails, fall back to the same honest `Unsupported` this
            // reported before the extension existed, never a silent
            // guess that it's there.
            ActiveWindowBackend::Unsupported(UnsupportedReason::GnomeRequiresShellExtension) => {
                // Self-heal once at startup — see gnome_extension::try_enable's
                // own doc comment for why this is more reliable than the
                // package postinst's attempt at the exact same thing.
                gnome_extension::try_enable();
                match GnomeExtensionSession::connect() {
                    Ok(session) => match session.focused_window_pid() {
                        Ok(_) => ActiveWindowSource::GnomeExtension(session),
                        Err(err) => {
                            self.last_gnome_error = Some(err.to_string());
                            ActiveWindowSource::Unsupported(UnsupportedReason::GnomeRequiresShellExtension)
                        }
                    },
                    Err(err) => {
                        self.last_gnome_error = Some(err.to_string());
                        ActiveWindowSource::Unsupported(UnsupportedReason::GnomeRequiresShellExtension)
                    }
                }
            }
            ActiveWindowBackend::Unsupported(UnsupportedReason::KdeRequiresKWinScript) => {
                match start_kwin_script() {
                    Ok(session) => ActiveWindowSource::KWinScript(session),
                    Err(err) => {
                        self.last_kwin_error = Some(err);
                        ActiveWindowSource::Unsupported(UnsupportedReason::KdeRequiresKWinScript)
                    }
                }
            }
            ActiveWindowBackend::Unsupported(reason) => ActiveWindowSource::Unsupported(reason),
        });

        match EvdevInputMonitor::start(Arc::clone(&self.input_counters)) {
            Ok(monitor) => self.evdev_monitor = Some(monitor),
            Err(_) => self.evdev_monitor = None, // no input-count capability this session — poll() reports zero counts, not an error
        }

        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut monitor) = self.evdev_monitor.take() {
            monitor.stop();
        }
    }

    fn poll(&mut self) -> RawSignalSnapshot {
        let (keyboard_events, mouse_move_events, mouse_click_events) =
            self.input_counters.take_and_reset();

        // Retry the GNOME extension probe on every poll while it's the
        // known-missing reason -- `start()` only gets one shot at this,
        // but the agent commonly starts via autostart at the exact
        // moment GNOME Shell itself is still loading extensions after
        // login, a real race with no guaranteed ordering (found by
        // actually installing on a real machine: the extension reported
        // ACTIVE and answered a manual D-Bus call correctly minutes
        // after the agent had already started and permanently cached
        // "unsupported" for that run). A single failed probe at startup
        // must not mean "unsupported for this entire run" when the
        // extension keeps genuinely working once it finishes loading.
        // The D-Bus round trip this costs (one call, only while
        // genuinely unsupported for this specific reason) is negligible
        // next to `POLL_INTERVAL`.
        if matches!(
            self.source,
            Some(ActiveWindowSource::Unsupported(
                UnsupportedReason::GnomeRequiresShellExtension
            ))
        ) {
            match GnomeExtensionSession::connect() {
                Ok(session) => match session.focused_window_pid() {
                    Ok(_) => {
                        self.last_gnome_error = None;
                        self.source = Some(ActiveWindowSource::GnomeExtension(session));
                    }
                    Err(err) => self.last_gnome_error = Some(err.to_string()),
                },
                Err(err) => self.last_gnome_error = Some(err.to_string()),
            }
        }

        // Same race, same fix, for KDE: KWin finishes loading its scripts
        // after the agent's own autostart, so one failed attempt at
        // startup must not mean "unsupported for this whole run". The
        // retry costs one process spawn plus two D-Bus calls per poll, and
        // only while this specific reason is the active one.
        if matches!(
            self.source,
            Some(ActiveWindowSource::Unsupported(
                UnsupportedReason::KdeRequiresKWinScript
            ))
        ) {
            match start_kwin_script() {
                Ok(session) => {
                    self.last_kwin_error = None;
                    self.source = Some(ActiveWindowSource::KWinScript(session));
                }
                Err(err) => self.last_kwin_error = Some(err),
            }
        }

        let (active_process_name, idle_seconds) = match &self.source {
            Some(ActiveWindowSource::X11(session)) => {
                let pid = session.active_window_pid().ok().flatten();
                let process_name = pid.and_then(process_name_for_pid);
                let idle = session
                    .idle_seconds()
                    .unwrap_or(self.input_counters.idle_seconds());
                (process_name, idle)
            }
            Some(ActiveWindowSource::Hyprland(path)) => {
                let pid = hyprland::active_window_pid(path).ok().flatten();
                let process_name = pid.and_then(process_name_for_pid);
                (process_name, self.input_counters.idle_seconds())
            }
            Some(ActiveWindowSource::GnomeExtension(session)) => {
                // No GNOME-specific idle source is wired up in this
                // collector (see AGENT_LINUX_CAPABILITY_MATRIX.md's
                // "likely supported, unverified" note on Mutter's own
                // IdleMonitor) — the evdev-based fallback already used
                // for Hyprland/Wayland generally applies here too.
                let pid = session.focused_window_pid().ok().flatten();
                let process_name = pid.and_then(process_name_for_pid);
                (process_name, self.input_counters.idle_seconds())
            }
            Some(ActiveWindowSource::KWinScript(session)) => {
                // The script pushes, so the newest value is already in
                // hand — but a value alone can't tell a quiet desktop
                // apart from a script that stopped running (KWin
                // restarted, the user switched it off in System Settings,
                // the script threw). Ask KWin, and on anything but a
                // clear "yes" re-enable it and report nothing for this
                // tick rather than re-serving a push that may be old.
                let pid = match session.is_script_loaded() {
                    Ok(true) => session.focused_window_pid(),
                    _ => {
                        session.try_enable();
                        None
                    }
                };
                (
                    pid.and_then(process_name_for_pid),
                    self.input_counters.idle_seconds(),
                )
            }
            Some(ActiveWindowSource::Unsupported(_)) | None => {
                (None, self.input_counters.idle_seconds())
            }
        };

        let is_idle = idle_seconds >= self.idle_threshold_seconds;

        // Title classification is still genuinely out of scope on Linux
        // (no window-title read exists here at all, unlike Windows — see
        // `windows_collector::browser_title`'s doc comment on why that's
        // the one place titles are ever examined there). URL rules are a
        // separate signal this platform CAN support, via AT-SPI2 —
        // see `browser_url.rs`'s doc comment for its real verification
        // status (unverified against a live browser in this round).
        let matched: Option<(String, String)> = match &active_process_name {
            Some(process_name)
                if should_classify_via_url(process_name, &self.browser_process_names, &self.browser_url_rules) =>
            {
                classify_browser_url(&mut self.address_bar_reader, process_name, &self.browser_url_rules)
                    .map(|(category, keyword)| (category, format!("url:{keyword}")))
            }
            _ => None,
        };
        let category_override = matched.as_ref().map(|(category, _)| category.clone());
        let matched_rule_key = matched.map(|(_, rule_key)| rule_key);

        // Company Layer — second, independent classification pass, see
        // `windows_collector::collector`'s poll() for the shared
        // reasoning.
        let company_category: Option<String> = match &active_process_name {
            Some(process_name)
                if should_classify_via_url(process_name, &self.browser_process_names, &self.company_browser_url_rules) =>
            {
                classify_browser_url(&mut self.address_bar_reader, process_name, &self.company_browser_url_rules)
                    .map(|(category, _keyword)| category)
            }
            _ => None,
        };

        // Opt-in personal domain tally — see
        // `windows_collector::collector`'s poll() for the shared reasoning
        // on why this is its own gate, not `should_classify_via_url`.
        let domain_host: Option<String> = if self.domain_tracking_enabled {
            match &active_process_name {
                Some(process_name) if self.browser_process_names.contains(&process_name.to_lowercase()) => self
                    .address_bar_reader
                    .read(process_name)
                    .and_then(|raw| normalization::extract_host(&raw))
                    // See windows-collector's identical gate for the real
                    // leak (a mid-typed search phrase read back as the
                    // address bar's literal text) `looks_like_a_domain`
                    // exists to filter out.
                    .filter(|host| normalization::looks_like_a_domain(host)),
                _ => None,
            }
        } else {
            None
        };

        RawSignalSnapshot {
            active_process_name,
            keyboard_events,
            mouse_move_events,
            mouse_click_events,
            is_idle,
            idle_seconds,
            category_override,
            matched_rule_key,
            company_category,
            domain_host,
        }
    }
}
