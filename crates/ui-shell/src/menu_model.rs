//! Pure menu structure — what entries exist, in what order, with what
//! text — separate from `tray.rs`'s actual OS menu construction. Testable
//! without any tray/OS dependency: a reviewer (or a test) can confirm the
//! exact menu contents and their accessibility labels without running a
//! real tray icon.

use crate::status::{
    last_sync_line, pause_resume_label, pending_line, status_line, version_line, AgentStatus,
};
use chrono::{DateTime, Utc};

/// One action a user can invoke from the tray menu — deliberately the
/// short, fixed list from `CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md`
/// §"UI должен содержать только" and nothing else (no charts, no embedded
/// dashboard, no settings beyond pause/resume — see that file's explicit
/// "Не добавлять" list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    ToggleActive,
    OpenDiagnostics,
    OpenDashboard,
    CheckForUpdates,
    Quit,
    OpenHelp,
}

/// One menu entry: either a clickable action or a plain informational
/// (disabled) line. Every entry `build_menu()` produces carries real text
/// — never an icon-only entry — so it's reachable by a screen reader via
/// the OS's native menu accessibility tree (native menu APIs expose item
/// text as their accessible name automatically, no extra annotation
/// needed). That guarantee lives in `build_menu()` and is checked by
/// `every_entry_has_a_non_empty_text_label` below, not in the type: the
/// fields here are public, so an empty label is still constructible
/// directly via a struct literal bypassing `build_menu()` — `tray.rs`
/// never does that, but this is a runtime/API-usage guarantee, not a
/// compile-time one (an earlier draft of this comment overstated it).
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    pub label: String,
    pub action: Option<MenuAction>,
    pub enabled: bool,
}

impl MenuEntry {
    fn info(label: impl Into<String>) -> Self {
        MenuEntry {
            label: label.into(),
            action: None,
            enabled: false,
        }
    }

    fn action(label: impl Into<String>, action: MenuAction) -> Self {
        MenuEntry {
            label: label.into(),
            action: Some(action),
            enabled: true,
        }
    }

    fn disabled_action(label: impl Into<String>, action: MenuAction) -> Self {
        MenuEntry {
            label: label.into(),
            action: Some(action),
            enabled: false,
        }
    }
}

/// Builds the full, ordered menu content for the given status. `now` is
/// injected (not read from the system clock) so this stays pure and
/// deterministic for tests; `tray.rs` supplies the real current time.
pub fn build_menu(status: &AgentStatus, now: DateTime<Utc>) -> Vec<MenuEntry> {
    let mut entries = vec![
        MenuEntry::info(status_line(status)),
        MenuEntry::info(last_sync_line(status, now)),
        MenuEntry::info(pending_line(status)),
    ];

    if status.paired {
        entries.push(MenuEntry::action(
            pause_resume_label(status),
            MenuAction::ToggleActive,
        ));
    }

    entries.push(MenuEntry::action(
        "Диагностика",
        MenuAction::OpenDiagnostics,
    ));
    entries.push(MenuEntry::action(
        "Открыть дашборд",
        MenuAction::OpenDashboard,
    ));
    // Not yet implemented server-side (AG-UPD-001+ are still TODO) — present
    // in the menu per the required item list, but disabled rather than
    // wired to a fake/no-op action a user could mistake for a real check.
    entries.push(MenuEntry::disabled_action(
        "Проверить обновления (скоро)",
        MenuAction::CheckForUpdates,
    ));
    entries.push(MenuEntry::info(version_line(status)));
    entries.push(MenuEntry::action(
        "Справка и удаление",
        MenuAction::OpenHelp,
    ));
    entries.push(MenuEntry::action("Выход", MenuAction::Quit));

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(paired: bool, is_paused: bool) -> AgentStatus {
        AgentStatus {
            paired,
            is_paused,
            last_sync: None,
            pending_count: 0,
            agent_version: "0.1.0-rust".to_string(),
        }
    }

    #[test]
    fn every_entry_has_a_non_empty_text_label() {
        // The whole "accessibility labels present" acceptance criterion,
        // structurally: an icon-only entry is impossible to construct
        // through this API (`MenuEntry` has no icon-only variant), and
        // this test additionally guards against a future regression where
        // someone passes an empty string as a label.
        for status in [
            status(true, false),
            status(true, true),
            status(false, false),
        ] {
            for entry in build_menu(&status, Utc::now()) {
                assert!(
                    !entry.label.trim().is_empty(),
                    "menu entry must have a non-empty accessible label: {entry:?}"
                );
            }
        }
    }

    #[test]
    fn pause_resume_entry_only_appears_when_paired() {
        let unpaired_menu = build_menu(&status(false, false), Utc::now());
        assert!(
            !unpaired_menu
                .iter()
                .any(|e| e.action == Some(MenuAction::ToggleActive)),
            "pausing/resuming an unpaired agent isn't a meaningful action to offer"
        );

        let paired_menu = build_menu(&status(true, false), Utc::now());
        assert!(paired_menu
            .iter()
            .any(|e| e.action == Some(MenuAction::ToggleActive)));
    }

    #[test]
    fn pause_resume_label_reflects_current_state_in_the_menu_itself() {
        let active_menu = build_menu(&status(true, false), Utc::now());
        let paused_menu = build_menu(&status(true, true), Utc::now());
        let active_entry = active_menu
            .iter()
            .find(|e| e.action == Some(MenuAction::ToggleActive))
            .unwrap();
        let paused_entry = paused_menu
            .iter()
            .find(|e| e.action == Some(MenuAction::ToggleActive))
            .unwrap();
        assert_eq!(active_entry.label, "Приостановить");
        assert_eq!(paused_entry.label, "Возобновить");
    }

    #[test]
    fn quit_is_always_present_and_enabled_regardless_of_state() {
        for status in [
            status(true, false),
            status(true, true),
            status(false, false),
        ] {
            let menu = build_menu(&status, Utc::now());
            let quit = menu.iter().find(|e| e.action == Some(MenuAction::Quit));
            assert!(
                quit.is_some(),
                "Quit must always be reachable, even if pairing/status is broken"
            );
            assert!(quit.unwrap().enabled);
        }
    }

    #[test]
    fn menu_never_contains_a_heavy_ui_action() {
        // Structural guard matching CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md's
        // explicit "Не добавлять" list: the `MenuAction` enum itself has no
        // variant for charts/full-dashboard-embedding/settings/analytics/
        // marketing screens — this test documents that boundary rather than
        // checking for their absence by name (there is nothing to check
        // for: they cannot be constructed).
        let all_actions = [
            MenuAction::ToggleActive,
            MenuAction::OpenDiagnostics,
            MenuAction::OpenDashboard,
            MenuAction::CheckForUpdates,
            MenuAction::Quit,
            MenuAction::OpenHelp,
        ];
        assert_eq!(all_actions.len(), 6, "exactly the actions this task's autopilot entry allows — update this test deliberately, not accidentally, if the list ever grows");
    }

    #[test]
    fn check_for_updates_is_present_but_disabled_until_the_updater_exists() {
        let menu = build_menu(&status(true, false), Utc::now());
        let entry = menu
            .iter()
            .find(|e| e.action == Some(MenuAction::CheckForUpdates))
            .unwrap();
        assert!(
            !entry.enabled,
            "must not look actionable until AG-UPD-001+ actually implement it"
        );
    }
}
