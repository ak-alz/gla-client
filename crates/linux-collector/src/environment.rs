//! Pure environment detection, deliberately taking env-var values as
//! plain `Option<&str>` parameters (not reading `std::env` itself) so
//! the decision logic is testable without a real session — mirrors this
//! project's established split (e.g. `windows_collector::idle::
//! idle_seconds_from_ticks`) between pure decision logic and the thin OS
//! read that feeds it.
//!
//! Per AG-LNX-001's capability matrix and the user's explicit scoping
//! decision for AG-LNX-002: X11 and Hyprland get real backends here.
//! GNOME is detected as needing its companion Shell extension by THIS
//! pure function — `collector.rs` is the layer that actually probes
//! whether that extension is installed/loaded and only then falls back
//! to reporting it unsupported. KDE and any other/unrecognized Wayland
//! compositor report an explicit unsupported status rather than
//! guessing or silently returning stale data — "Missing capability
//! returns explicit status" is this task's own acceptance criterion,
//! not an afterthought.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveWindowBackend {
    X11,
    Hyprland,
    /// A real, named reason this session's compositor isn't supported —
    /// always carries a human-readable reason so a caller (e.g. the
    /// tray's diagnostics view) can show a real explanation, never a bare
    /// "unavailable."
    Unsupported(UnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedReason {
    /// GNOME Shell's `org.gnome.Shell.Eval` is gated behind unsafe mode
    /// since GNOME 41 — the companion Shell extension
    /// (`installer/linux/gnome-extension/`, `gnome_extension.rs`) closes
    /// this, but only once it's actually installed AND loaded (a fresh
    /// install needs one log out/in on Wayland — see the extension's own
    /// metadata). This variant means the extension didn't respond right
    /// now, for any of those reasons — `collector.rs` is the layer that
    /// actually attempts the D-Bus call before falling back to this.
    /// See AGENT_LINUX_CAPABILITY_MATRIX.md.
    GnomeRequiresShellExtension,
    /// KWin scripting requires a loaded KWin script (not shipped by this
    /// task) — see AGENT_LINUX_CAPABILITY_MATRIX.md.
    KdeRequiresKWinScript,
    /// A Wayland compositor other than GNOME/KDE/Hyprland — no generic
    /// active-window API exists (Wayland's client-isolation design), and
    /// this task did not add a backend for it specifically.
    OtherWaylandCompositor(String),
    /// `$XDG_SESSION_TYPE` was empty, unset, or an unrecognized value —
    /// deliberately distinct from "recognized Wayland compositor we
    /// don't support," since it likely indicates a detection bug rather
    /// than a genuinely unsupported environment.
    UnknownSessionType,
}

/// Decides which active-window backend to use from the same environment
/// variables every desktop-environment-aware Linux tool relies on
/// (`$XDG_SESSION_TYPE`, `$XDG_CURRENT_DESKTOP`, and Hyprland's own
/// `$HYPRLAND_INSTANCE_SIGNATURE`, which is the standard way Hyprland
/// itself advertises "a Hyprland compositor is running and reachable at
/// this IPC socket" — see the Hyprland wiki's IPC documentation).
pub fn detect_active_window_backend(
    xdg_session_type: Option<&str>,
    xdg_current_desktop: Option<&str>,
    hyprland_instance_signature: Option<&str>,
) -> ActiveWindowBackend {
    if hyprland_instance_signature.is_some_and(|s| !s.is_empty()) {
        return ActiveWindowBackend::Hyprland;
    }

    match xdg_session_type.map(|s| s.to_lowercase()).as_deref() {
        Some("x11") => ActiveWindowBackend::X11,
        Some("wayland") => {
            let desktop = xdg_current_desktop.unwrap_or("").to_lowercase();
            if desktop.contains("gnome") {
                ActiveWindowBackend::Unsupported(UnsupportedReason::GnomeRequiresShellExtension)
            } else if desktop.contains("kde") || desktop.contains("plasma") {
                ActiveWindowBackend::Unsupported(UnsupportedReason::KdeRequiresKWinScript)
            } else {
                ActiveWindowBackend::Unsupported(UnsupportedReason::OtherWaylandCompositor(
                    xdg_current_desktop.unwrap_or("(unknown)").to_string(),
                ))
            }
        }
        _ => ActiveWindowBackend::Unsupported(UnsupportedReason::UnknownSessionType),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprland_signature_wins_even_under_a_wayland_session_type() {
        // Hyprland sessions report XDG_SESSION_TYPE=wayland too — the
        // Hyprland-specific signature must take priority over generic
        // Wayland/desktop-name sniffing, not fall through to "other".
        assert_eq!(
            detect_active_window_backend(Some("wayland"), Some("Hyprland"), Some("abc123")),
            ActiveWindowBackend::Hyprland
        );
    }

    #[test]
    fn plain_x11_session() {
        assert_eq!(
            detect_active_window_backend(Some("x11"), None, None),
            ActiveWindowBackend::X11
        );
        assert_eq!(
            detect_active_window_backend(Some("X11"), None, None),
            ActiveWindowBackend::X11,
            "session type comparison must be case-insensitive"
        );
    }

    #[test]
    fn gnome_wayland_is_explicitly_unsupported() {
        assert_eq!(
            detect_active_window_backend(Some("wayland"), Some("GNOME"), None),
            ActiveWindowBackend::Unsupported(UnsupportedReason::GnomeRequiresShellExtension)
        );
    }

    #[test]
    fn kde_wayland_is_explicitly_unsupported() {
        assert_eq!(
            detect_active_window_backend(Some("wayland"), Some("KDE"), None),
            ActiveWindowBackend::Unsupported(UnsupportedReason::KdeRequiresKWinScript)
        );
        assert_eq!(
            detect_active_window_backend(Some("wayland"), Some("plasma"), None),
            ActiveWindowBackend::Unsupported(UnsupportedReason::KdeRequiresKWinScript)
        );
    }

    #[test]
    fn unrecognized_wayland_compositor_names_itself_in_the_reason() {
        let result = detect_active_window_backend(Some("wayland"), Some("sway"), None);
        assert_eq!(
            result,
            ActiveWindowBackend::Unsupported(UnsupportedReason::OtherWaylandCompositor(
                "sway".to_string()
            ))
        );
    }

    #[test]
    fn missing_or_unrecognized_session_type_is_unknown_not_a_silent_default() {
        assert_eq!(
            detect_active_window_backend(None, None, None),
            ActiveWindowBackend::Unsupported(UnsupportedReason::UnknownSessionType)
        );
        assert_eq!(
            detect_active_window_backend(Some("tty"), None, None),
            ActiveWindowBackend::Unsupported(UnsupportedReason::UnknownSessionType)
        );
    }
}
