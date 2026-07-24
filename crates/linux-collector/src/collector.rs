//! `LinuxSignalCollector` — ties `environment`/`x11`/`hyprland`/
//! `evdev_counter`/`process_name` together into `collector_core::
//! SignalCollector`, mirroring `windows_collector::WindowsSignalCollector`'s
//! role exactly: `start()` detects the environment and starts whichever
//! backend applies, `poll()` returns one `RawSignalSnapshot`.

use crate::environment::{detect_active_window_backend, ActiveWindowBackend, UnsupportedReason};
use crate::evdev_counter::EvdevInputMonitor;
use crate::gnome_extension::GnomeExtensionSession;
use crate::hyprland;
use crate::input_counters::InputCounters;
use crate::process_name::process_name_for_pid;
use crate::x11::{X11Error, X11Session};
use collector_core::{RawSignalSnapshot, SignalCollector};
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
    Unsupported(UnsupportedReason),
}

pub struct LinuxSignalCollector {
    idle_threshold_seconds: f64,
    source: Option<ActiveWindowSource>,
    input_counters: Arc<InputCounters>,
    evdev_monitor: Option<EvdevInputMonitor>,
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
}

impl LinuxSignalCollector {
    pub fn new(idle_threshold_seconds: f64) -> Self {
        LinuxSignalCollector {
            idle_threshold_seconds,
            source: None,
            input_counters: Arc::new(InputCounters::new()),
            evdev_monitor: None,
            last_gnome_error: None,
        }
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
            Some(ActiveWindowSource::Unsupported(_)) | None => {
                (None, self.input_counters.idle_seconds())
            }
        };

        let is_idle = idle_seconds >= self.idle_threshold_seconds;

        RawSignalSnapshot {
            active_process_name,
            keyboard_events,
            mouse_move_events,
            mouse_click_events,
            is_idle,
            idle_seconds,
            category_override: None, // browser-title classification: not in this task's scope
        }
    }
}
