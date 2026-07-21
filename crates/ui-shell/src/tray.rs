//! Real OS tray integration — the only module in this crate that touches
//! actual native APIs (tray-icon + winit), and therefore the only one NOT
//! covered by automated tests (there is no meaningful way to unit-test
//! "does a real system tray icon appear" in a headless test run). See
//! TEST_REPORT.md for how this was verified instead: by actually running
//! it and inspecting the resulting process.
//!
//! Per ADR 0013 (binding, not a suggestion): NO window of any kind is
//! created here, ever, at startup or otherwise — `TrayIconBuilder` is the
//! only UI object this module constructs. "Open dashboard"/"Diagnostics"
//! shell out to the system browser (see `browser.rs`) instead of creating
//! a webview window, which sidesteps the ~353 MB Tauri-hidden-window
//! regression ADR 0013 documents entirely, by construction.

use crate::menu_model::{build_menu, MenuAction, MenuEntry};
use crate::status::AgentStatus;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
#[cfg(not(target_os = "linux"))]
use winit::application::ApplicationHandler;
#[cfg(not(target_os = "linux"))]
use winit::event::WindowEvent;
#[cfg(not(target_os = "linux"))]
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(not(target_os = "linux"))]
use winit::window::WindowId;

/// The seam between this crate's tray plumbing and whatever process
/// actually drives the collector/queue/uploader (a future integration
/// task, e.g. AG-008's service wiring). Implementing this trait is the
/// ONLY thing a caller needs to do to use this tray shell — this crate
/// never assumes a specific collector/queue implementation, matching
/// AG-006/AG-003's "shared, not per-platform" design.
pub trait AgentController: Send + Sync + 'static {
    fn status(&self) -> AgentStatus;
    fn toggle_active(&self);
    fn dashboard_url(&self) -> String;
    fn diagnostics_url(&self) -> String;
    fn help_url(&self) -> String;
    /// Real device-authorization pairing (added post-launch). Must
    /// return promptly — implementations spawn their own thread for the
    /// actual network calls/polling, never blocking this call site,
    /// which runs on the tray's own event-loop thread.
    fn pair_device(&self);
}

/// How often the menu's time-sensitive text (mainly "last sync: N minutes
/// ago") is refreshed even with no user interaction. A real tick, not
/// disabled — but coarse enough that reopening the menu practically never
/// causes visible mid-open flicker, without needing to detect "is the menu
/// currently open" (which tray-icon/winit don't expose portably).
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

fn make_icon(active: bool) -> Icon {
    // 16x16 solid-color RGBA icon generated in code, no external asset —
    // same approach already benchmarked in AG-002's rust-tray prototype.
    // Blue while actively syncing; gray whenever paused OR unpaired, so
    // the pause state is visible from the icon alone, not only from
    // opening the menu (the "Pause status заметен" acceptance criterion).
    let size = 16u32;
    let color = if active {
        [80u8, 140, 255, 255]
    } else {
        [140u8, 140, 140, 255]
    };
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        rgba.extend_from_slice(&color);
    }
    Icon::from_rgba(rgba, size, size).expect("16x16 RGBA buffer is always a valid icon")
}

struct BuiltMenu {
    menu: Menu,
    // Kept (not dropped after construction) so a later `refresh()` can
    // patch each item's text in place via `set_text` — see `App::refresh`'s
    // doc comment for the real, user-reported bug this exists to avoid.
    items: Vec<MenuItem>,
    action_by_id: HashMap<MenuId, MenuAction>,
}

fn build_native_menu(entries: &[MenuEntry]) -> BuiltMenu {
    let menu = Menu::new();
    let mut items = Vec::with_capacity(entries.len());
    let mut action_by_id = HashMap::new();

    for entry in entries {
        // Both actionable and informational entries are the same native
        // `MenuItem` (native menu APIs have no dedicated non-interactive
        // item type) — a disabled one is the standard idiom for a
        // "current status" line. Only actionable ones get an
        // `action_by_id` entry; both get their handle kept in `items` so
        // `refresh()` can update their text later.
        let item = MenuItem::new(&entry.label, entry.enabled, None);
        if let Some(action) = entry.action {
            action_by_id.insert(item.id().clone(), action);
        }
        let _ = menu.append(&item);
        items.push(item);
    }
    let _ = menu.append(&PredefinedMenuItem::separator());

    BuiltMenu { menu, items, action_by_id }
}

struct App {
    controller: Arc<dyn AgentController>,
    tray: TrayIcon,
    items: Vec<MenuItem>,
    // (action, enabled) per entry, in order — the STRUCTURAL shape of the
    // currently-displayed menu, as opposed to its text. Compared against
    // on each refresh to decide whether the native menu can be patched in
    // place or must be rebuilt from scratch (see `refresh()`'s doc comment).
    entry_shape: Vec<(Option<MenuAction>, bool)>,
    action_by_id: HashMap<MenuId, MenuAction>,
    last_status: AgentStatus,
}

impl App {
    /// Real, user-reported bug this avoids: the periodic 5-second refresh
    /// used to call `set_menu` with a BRAND NEW `Menu` (fresh native
    /// `MenuId`s) every single tick, unconditionally. If a user opened the
    /// tray menu and took longer than one tick to click something (utterly
    /// ordinary — reading "Последняя синхронизация: N назад" before
    /// deciding what to click), the periodic refresh silently replaced the
    /// menu-and-ids UNDERNEATH the still-visible native popup. The click
    /// then arrived tagged with an id from the now-discarded menu, missed
    /// every entry in `self.action_by_id` (which had already moved on to
    /// the new ids), and did nothing — exactly "кнопки меню перестали
    /// что-то делать вообще".
    ///
    /// Fix: only rebuild (and mint new ids) when the menu's actual
    /// STRUCTURE changed (an item was added/removed/enabled toggled —
    /// e.g. a pairing completing, "Приостановить"<->"Возобновить" does NOT
    /// count, only its text does). Otherwise patch each existing item's
    /// text in place via `set_text`, keeping the same ids a click made
    /// against the on-screen menu at any point will still resolve against.
    fn refresh(&mut self) {
        let status = self.controller.status();
        let entries = build_menu(&status, chrono::Utc::now());
        let shape: Vec<(Option<MenuAction>, bool)> =
            entries.iter().map(|e| (e.action, e.enabled)).collect();

        if shape == self.entry_shape {
            for (item, entry) in self.items.iter().zip(entries.iter()) {
                item.set_text(&entry.label);
            }
        } else {
            let built = build_native_menu(&entries);
            self.tray.set_menu(Some(Box::new(built.menu)));
            self.items = built.items;
            self.action_by_id = built.action_by_id;
            self.entry_shape = shape;
        }

        let is_active = status.paired && !status.is_paused;
        let was_active = self.last_status.paired && !self.last_status.is_paused;
        if is_active != was_active {
            let _ = self.tray.set_icon(Some(make_icon(is_active)));
        }
        self.last_status = status;
    }

    /// Returns `true` when `Quit` was chosen — deliberately does NOT take
    /// a winit `ActiveEventLoop` (unlike an earlier version of this
    /// method), so this same logic drives both the winit-based event
    /// loop (Windows/macOS) and the GTK-based one (Linux) below; each
    /// caller decides HOW to actually stop its own loop when this
    /// returns `true`.
    fn handle_action(&mut self, action: MenuAction) -> bool {
        match action {
            MenuAction::ToggleActive => {
                self.controller.toggle_active();
                self.refresh();
                false
            }
            MenuAction::OpenDiagnostics => {
                let _ = crate::browser::open_url(&self.controller.diagnostics_url());
                false
            }
            MenuAction::OpenDashboard => {
                let _ = crate::browser::open_url(&self.controller.dashboard_url());
                false
            }
            MenuAction::OpenHelp => {
                let _ = crate::browser::open_url(&self.controller.help_url());
                false
            }
            MenuAction::CheckForUpdates => {
                // Disabled in the menu until AG-UPD-001+ exist — a click
                // should be unreachable, but if one somehow arrives, doing
                // nothing is the safe default, not a crash or a fake success.
                false
            }
            MenuAction::PairDevice => {
                self.controller.pair_device();
                false
            }
            MenuAction::Quit => true,
        }
    }
}

/// Winit's own event loop is sufficient to pump `tray-icon` on Windows
/// and macOS (its native message/run loop already carries the tray's
/// events) — NOT on Linux, where `tray-icon`'s backend is GTK-based and
/// needs a real, separate GTK main loop the caller must drive (see the
/// `#[cfg(target_os = "linux")]` `run_tray` below and its doc comment).
#[cfg(not(target_os = "linux"))]
impl ApplicationHandler for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, _event: WindowEvent) {
        // No window is ever created by this crate — see module doc
        // comment — so there is deliberately nothing to handle here.
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + REFRESH_INTERVAL,
        ));

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(action) = self.action_by_id.get(&event.id).copied() {
                if self.handle_action(action) {
                    event_loop.exit();
                    return;
                }
            }
        }

        self.refresh();
    }
}

fn build_app(controller: Arc<dyn AgentController>) -> Result<App, Box<dyn std::error::Error>> {
    let initial_status = controller.status();
    let is_active = initial_status.paired && !initial_status.is_paused;

    let entries = build_menu(&initial_status, chrono::Utc::now());
    let entry_shape = entries.iter().map(|e| (e.action, e.enabled)).collect();
    let built = build_native_menu(&entries);

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(built.menu))
        .with_tooltip("Growth Layer")
        .with_icon(make_icon(is_active))
        .build()?;

    Ok(App {
        controller,
        tray,
        items: built.items,
        entry_shape,
        action_by_id: built.action_by_id,
        last_status: initial_status,
    })
}

/// Runs the tray shell on the calling thread until the user chooses Quit.
/// Blocks — callers that also need to run a background collector/uploader
/// loop must do so on a SEPARATE thread before calling this (see
/// `examples/tray_demo.rs` for a concrete demonstration that the tray and
/// the background loop are independent of one another, satisfying "UI не
/// нужен для работы collector"/"Collector продолжает работать без
/// открытого UI").
#[cfg(not(target_os = "linux"))]
pub fn run_tray(controller: Arc<dyn AgentController>) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = build_app(controller)?;
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Linux-only: `tray-icon`'s Linux backend is GTK/libayatana-appindicator-
/// based and requires a real, running GTK main loop on the SAME thread
/// that created the tray/menu (GTK objects are not thread-safe to touch
/// from elsewhere) — winit's own event loop does not provide this. A
/// real panic ("GTK has not been initialized. Call `gtk::init` first.")
/// was hit by actually running this crate for the first time on real
/// Linux (WSLg) in AG-LNX-003 — this is the fix, not a hypothetical
/// hardening. No separate thread is spawned for GTK here (unlike
/// `tray-icon`'s own winit example, which spawns one because IT also
/// runs a winit event loop with real windows on the main thread
/// simultaneously) — this crate never creates a winit window at all
/// (see the module doc comment's ADR 0013 note), so GTK's main loop can
/// simply run directly on the thread `run_tray` was called from.
#[cfg(target_os = "linux")]
pub fn run_tray(controller: Arc<dyn AgentController>) -> Result<(), Box<dyn std::error::Error>> {
    use std::cell::RefCell;
    use std::rc::Rc;

    gtk::init()?;

    let app = Rc::new(RefCell::new(build_app(controller)?));

    let timer_app = Rc::clone(&app);
    glib::source::timeout_add_local(REFRESH_INTERVAL, move || {
        let mut app = timer_app.borrow_mut();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(action) = app.action_by_id.get(&event.id).copied() {
                if app.handle_action(action) {
                    gtk::main_quit();
                    return glib::ControlFlow::Break;
                }
            }
        }
        app.refresh();
        glib::ControlFlow::Continue
    });

    gtk::main();
    Ok(())
}

/// Lets a caller outside the GTK thread (e.g. `agent-bin`'s SIGTERM/SIGINT
/// handler thread — see its doc comment for why `systemctl --user stop`
/// needs this) ask the tray loop to quit exactly as if the user had
/// clicked Quit. `MainContext::invoke` is the documented thread-safe way
/// to schedule a closure onto the thread that owns a given `MainContext`
/// (here, the default one `run_tray` runs on) — calling `gtk::main_quit()`
/// directly from another thread would not be safe, GTK/GLib objects are
/// not thread-safe to touch except through mechanisms built for exactly
/// this.
#[cfg(target_os = "linux")]
pub fn request_quit() {
    glib::MainContext::default().invoke(|| {
        gtk::main_quit();
    });
}
