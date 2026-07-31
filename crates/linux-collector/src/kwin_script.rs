//! Client side of a small companion KWin script
//! (`installer/linux/kwin-script/`) — the only way to learn which window
//! is focused on KDE Plasma Wayland, where no portal or public API
//! exposes it (see `environment.rs`'s doc comment).
//!
//! # Why this is the mirror image of `gnome_extension.rs`
//!
//! A KWin script written in plain JS cannot export a D-Bus object at
//! all: its whole API surface is `callDBus`/`registerShortcut`/
//! `readConfig`/`registerScreenEdge`/`registerUserActionsMenu`. So the
//! direction is inverted — the script PUSHES
//! `org.growthlayer.AgentKWinBridge.SetFocusedWindow` and THIS process
//! owns the bus name and serves the object, whereas on GNOME the
//! extension serves and the agent calls. Everything downstream is
//! unchanged: a PID goes into `process_name_for_pid`, exactly like the
//! X11/Hyprland/GNOME backends.
//!
//! Because the script cannot be asked anything, and has no timer to
//! announce itself with (no `setTimeout`; `QTimer` is QML-only), a
//! script that was already running before the agent started has already
//! pushed its one load-time message into the void. `try_enable` below
//! therefore RELOADS it rather than merely enabling it — that reload is
//! what makes it re-announce the current focus.
//!
//! # What was verified for real, and what wasn't
//!
//! Verified live on Plasma 6.7.3 (Wayland) with a throwaway probe
//! script: `workspace.activeWindow` yields `resourceClass` and a numeric
//! `pid` (`google-chrome`/`3267`); `[Plugins] <id>Enabled=true` in
//! `kwinrc` plus `org.kde.kwin.Scripting.start()` is what actually loads
//! a package from `~/.local/share/kwin/scripts/<id>/` (an explicit
//! `loadScript` call is NOT needed — discovery finds it); `unloadScript`
//! followed by `start()` genuinely re-runs the script (three cycles
//! produced three load messages in the journal); `isScriptLoaded`
//! reports `false` for an unloaded or unknown plugin id.
//!
//! NOT verified: what `workspace.windowActivated` passes when focus is
//! dropped entirely (the script's `if (!window)` guard covers it), how
//! `callDBus` marshals its arguments against this interface's `si`
//! signature, and whether XWayland clients always carry a usable PID.
//! Treat those as code-complete, not field-verified — the same honesty
//! `gnome_extension.rs`'s own module doc applies to the GNOME half.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use zbus::blocking::Connection;

pub const PLUGIN_ID: &str = "growthlayeragent";

const SERVICE: &str = "org.growthlayer.AgentKWinBridge";
const OBJECT_PATH: &str = "/org/growthlayer/AgentKWinBridge";
// The interface name itself lives in the `#[zbus::interface]` attribute
// below — the macro takes a literal, so a constant here would only be a
// second copy to keep in sync.

const KWIN_SERVICE: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_INTERFACE: &str = "org.kde.kwin.Scripting";

/// Where the packages install the script read-only; `try_enable` copies
/// it into the user's own KWin script dir, which is the only place KWin
/// looks for per-user packages. The tarball installer already copies it
/// there itself (it runs as the user), so a missing source dir here is
/// normal, not an error.
const SYSTEM_PACKAGE_DIR: &str = "/usr/share/growth-layer-agent/kwin-script";

type SharedFocus = Arc<Mutex<Option<(String, u32)>>>;

struct FocusSink {
    state: SharedFocus,
}

#[zbus::interface(name = "org.growthlayer.AgentKWinBridge")]
impl FocusSink {
    /// `pid` is `i32` to match what the script's `callDBus` sends (and
    /// the GNOME extension's own `i` out-arg) — a PID that is zero or
    /// negative means the same thing as an empty resource class: nothing
    /// usable is focused right now, which is `None`, never an error.
    fn set_focused_window(&self, resource_class: String, pid: i32) {
        let focus = u32::try_from(pid)
            .ok()
            .filter(|pid| *pid > 0 && !resource_class.is_empty())
            .map(|pid| (resource_class, pid));
        if let Ok(mut state) = self.state.lock() {
            *state = focus;
        }
    }
}

#[derive(Debug, Error)]
pub enum KWinScriptError {
    #[error("failed to serve {SERVICE} on the session bus (another agent instance may already own it): {0}")]
    Export(zbus::Error),
    #[error("KWin did not answer isScriptLoaded (not a KWin session, or KWin is restarting): {0}")]
    CallFailed(zbus::Error),
}

/// Owns the exported object for the whole run — dropping this stops
/// serving `SERVICE` and the script's pushes start failing, so it lives
/// as long as the collector, matching `GnomeExtensionSession`/
/// `X11Session`'s shape.
pub struct KWinScriptSession {
    conn: Connection,
    state: SharedFocus,
}

impl KWinScriptSession {
    pub fn start() -> Result<Self, KWinScriptError> {
        let state: SharedFocus = Arc::new(Mutex::new(None));
        // `name()` before `build()` on purpose: requesting the well-known
        // name as part of construction leaves no window in which the
        // script could push to a name nobody owns yet.
        let conn = zbus::blocking::connection::Builder::session()
            .map_err(KWinScriptError::Export)?
            .serve_at(
                OBJECT_PATH,
                FocusSink {
                    state: Arc::clone(&state),
                },
            )
            .map_err(KWinScriptError::Export)?
            .name(SERVICE)
            .map_err(KWinScriptError::Export)?
            .build()
            .map_err(KWinScriptError::Export)?;
        Ok(KWinScriptSession { conn, state })
    }

    /// `None` means nothing usable is focused — same `Option` semantics
    /// as `x11::X11Session::active_window_pid` and
    /// `gnome_extension::GnomeExtensionSession::focused_window_pid`. A
    /// PID that has since exited is filtered out downstream by
    /// `process_name_for_pid`, so a script that stopped pushing can
    /// never turn into a stale attribution.
    pub fn focused_window_pid(&self) -> Option<u32> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.as_ref().map(|(_, pid)| *pid))
    }

    /// The liveness check for this backend. Deliberately NOT
    /// `focused_window_pid` (which legitimately answers `None` on an
    /// empty desktop): only KWin itself knows whether the script is
    /// still loaded after a KWin restart, a crash in the script, or the
    /// user switching it off in System Settings.
    pub fn is_script_loaded(&self) -> Result<bool, KWinScriptError> {
        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                SCRIPTING_PATH,
                Some(SCRIPTING_INTERFACE),
                "isScriptLoaded",
                &(PLUGIN_ID,),
            )
            .map_err(KWinScriptError::CallFailed)?;
        reply
            .body()
            .deserialize()
            .map_err(KWinScriptError::CallFailed)
    }

    /// Install (if needed), enable and reload the script, from inside the
    /// agent's own process — the same reasoning as
    /// `gnome_extension::try_enable`'s doc comment: unlike a package
    /// postinst reaching in through `runuser`, this code is guaranteed to
    /// already be in a real user session. Idempotent, and a harmless
    /// no-op on a non-KDE desktop (the D-Bus calls simply fail).
    ///
    /// Order matters: enable first so a future KWin start picks the
    /// script up on its own, then reload so it re-announces the current
    /// focus to a bus name that (by construction) already exists.
    pub fn try_enable(&self) {
        if let Some(dir) = user_script_dir(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        ) {
            let _ = install_user_package(Path::new(SYSTEM_PACKAGE_DIR), &dir);
        }

        let _ = Command::new("kwriteconfig6")
            .args([
                "--file",
                "kwinrc",
                "--group",
                "Plugins",
                "--key",
                &kwinrc_enabled_key(PLUGIN_ID),
                "true",
            ])
            .status();

        let _ = self.conn.call_method(
            Some(KWIN_SERVICE),
            SCRIPTING_PATH,
            Some(SCRIPTING_INTERFACE),
            "unloadScript",
            &(PLUGIN_ID,),
        );
        let _ = self.conn.call_method(
            Some(KWIN_SERVICE),
            SCRIPTING_PATH,
            Some(SCRIPTING_INTERFACE),
            "start",
            &(),
        );
    }
}

/// `[Plugins] <id>Enabled` is the key KWin's own script KCM writes — the
/// plugin id is used verbatim, which is why `PLUGIN_ID` has no hyphens.
fn kwinrc_enabled_key(plugin_id: &str) -> String {
    format!("{plugin_id}Enabled")
}

/// `~/.local/share/kwin/scripts/<id>/` (or `$XDG_DATA_HOME`'s
/// equivalent) — verified with `kpackagetool6 --type KWin/Script --list`
/// as the per-user package root. Takes both env values as parameters
/// rather than reading them itself, matching `environment.rs`'s split
/// between pure logic and the thin OS read that feeds it.
fn user_script_dir(xdg_data_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let data_home = match xdg_data_home.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|dir| !dir.is_empty())?).join(".local/share"),
    };
    Some(data_home.join("kwin/scripts").join(PLUGIN_ID))
}

/// KWin appends exactly `/contents/code/main.js` to a package dir (the
/// literal is in `libkwin.so.6`), so the layout is copied as-is.
fn main_js_path(package_dir: &Path) -> PathBuf {
    package_dir.join("contents/code/main.js")
}

fn install_user_package(system_dir: &Path, user_dir: &Path) -> std::io::Result<()> {
    let metadata_src = system_dir.join("metadata.json");
    let main_js_src = main_js_path(system_dir);
    if !metadata_src.is_file() || !main_js_src.is_file() {
        return Ok(());
    }
    let main_js_dest = main_js_path(user_dir);
    std::fs::create_dir_all(main_js_dest.parent().expect("main.js always has a parent"))?;
    std::fs::copy(&metadata_src, user_dir.join("metadata.json"))?;
    std::fs::copy(&main_js_src, &main_js_dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_key_is_the_plugin_id_verbatim() {
        // Verified live: this exact key in kwinrc's [Plugins] group is
        // what makes Scripting.start() load the package.
        assert_eq!(kwinrc_enabled_key(PLUGIN_ID), "growthlayeragentEnabled");
    }

    #[test]
    fn user_script_dir_prefers_xdg_data_home_then_falls_back_to_home() {
        assert_eq!(
            user_script_dir(Some("/data"), Some("/home/u")),
            Some(PathBuf::from("/data/kwin/scripts/growthlayeragent"))
        );
        assert_eq!(
            user_script_dir(None, Some("/home/u")),
            Some(PathBuf::from(
                "/home/u/.local/share/kwin/scripts/growthlayeragent"
            ))
        );
        assert_eq!(
            user_script_dir(Some(""), Some("/home/u")),
            Some(PathBuf::from(
                "/home/u/.local/share/kwin/scripts/growthlayeragent"
            )),
            "an empty XDG_DATA_HOME must not produce a relative path"
        );
        assert_eq!(user_script_dir(None, None), None);
    }

    #[test]
    fn main_js_path_matches_the_layout_kwin_expects() {
        assert!(main_js_path(Path::new("/pkg")).ends_with("contents/code/main.js"));
    }

    #[test]
    fn a_push_without_a_usable_window_reads_back_as_nothing_focused() {
        let state: SharedFocus = Arc::new(Mutex::new(None));
        let sink = FocusSink {
            state: Arc::clone(&state),
        };

        sink.set_focused_window("google-chrome".to_string(), 3267);
        assert_eq!(
            *state.lock().unwrap(),
            Some(("google-chrome".to_string(), 3267))
        );

        // The script's own "no window focused" signal, and the two
        // malformed cases it must not be confused with.
        sink.set_focused_window(String::new(), 0);
        assert_eq!(*state.lock().unwrap(), None);

        sink.set_focused_window("konsole".to_string(), 0);
        assert_eq!(
            *state.lock().unwrap(),
            None,
            "a zero PID is never a real window, whatever the resource class says"
        );

        sink.set_focused_window("konsole".to_string(), -1);
        assert_eq!(*state.lock().unwrap(), None);
    }

    #[test]
    fn each_push_replaces_the_previous_focus() {
        let state: SharedFocus = Arc::new(Mutex::new(None));
        let sink = FocusSink {
            state: Arc::clone(&state),
        };
        sink.set_focused_window("konsole".to_string(), 10);
        sink.set_focused_window("firefox".to_string(), 20);
        assert_eq!(*state.lock().unwrap(), Some(("firefox".to_string(), 20)));
    }

    #[test]
    fn install_user_package_is_a_no_op_without_a_system_package() {
        let user_dir = std::env::temp_dir().join(format!(
            "gla-kwin-test-{}-{}",
            std::process::id(),
            "absent-source"
        ));
        assert!(install_user_package(Path::new("/nonexistent/kwin-script"), &user_dir).is_ok());
        assert!(
            !user_dir.exists(),
            "nothing should be created when the packaged script isn't installed (tarball case)"
        );
    }

    #[test]
    fn install_user_package_copies_both_files_preserving_the_layout() {
        let base = std::env::temp_dir().join(format!("gla-kwin-test-{}-copy", std::process::id()));
        let system_dir = base.join("system");
        let user_dir = base.join("user");
        std::fs::create_dir_all(main_js_path(&system_dir).parent().unwrap()).unwrap();
        std::fs::write(system_dir.join("metadata.json"), "{}").unwrap();
        std::fs::write(main_js_path(&system_dir), "// script").unwrap();

        install_user_package(&system_dir, &user_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(main_js_path(&user_dir)).unwrap(),
            "// script"
        );
        assert!(user_dir.join("metadata.json").is_file());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
