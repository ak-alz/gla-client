//! Pure status data and formatting — no OS/tray dependency, fully unit
//! testable. `tray.rs` is the only module allowed to touch actual OS APIs;
//! everything here is plain data and string formatting.

use chrono::{DateTime, Utc};

/// A snapshot of what the tray should currently show. Supplied by whatever
/// process actually runs the collector/uploader (a future integration
/// task, e.g. AG-008's service wiring) — this crate defines the shape of
/// that status, not how it gets produced.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatus {
    pub paired: bool,
    pub is_paused: bool,
    /// `None` means "never synced yet" — distinct from "synced a very long
    /// time ago," which still has a real timestamp.
    pub last_sync: Option<DateTime<Utc>>,
    pub pending_count: usize,
    pub agent_version: String,
    /// `Some(version)` when a real, verified, in-rollout update has been
    /// found (see `agent-bin`'s `update_check` module) — a plain string,
    /// not a `semver::Version`, so this crate (presentation-only, no
    /// knowledge of the agent's internals) never needs to depend on
    /// `semver` just to display a version number.
    pub available_update_version: Option<String>,
}

/// Mirrors the running/paused distinction as a short, unambiguous label —
/// used both in the menu text and to pick which tray icon color to show
/// (see `tray.rs`), so pause state is visible in two independent places,
/// not just a menu item someone has to open the menu to notice.
pub fn status_line(status: &AgentStatus) -> String {
    match (status.paired, status.is_paused) {
        (false, _) => "Не привязано".to_string(),
        (true, true) => "Приостановлено".to_string(),
        (true, false) => "Работает".to_string(),
    }
}

/// The tray icon's hover tooltip — real feedback: the menu already shows
/// `status_line` once opened, but a glance at the tray icon itself (no
/// click) showed nothing but the bare app name. An available update wins
/// over the plain running/paused state — it's the one thing worth
/// surfacing on a passive hover, the running/paused distinction is
/// already visible via the icon's own dimming (see `icons.rs`). Not the
/// same wording as `status_line` on purpose — this is a short tooltip
/// line, not the fuller menu-item label.
pub fn tooltip_line(status: &AgentStatus) -> String {
    if status.available_update_version.is_some() {
        return "DevPace — есть обновление".to_string();
    }
    if !status.paired {
        return "DevPace — не привязано".to_string();
    }
    if status.is_paused {
        "DevPace — остановлен".to_string()
    } else {
        "DevPace — работает".to_string()
    }
}

/// Formats `last_sync` relative to `now` in coarse, human units — mirrors
/// the kind of relative-time label already used elsewhere in this product
/// (e.g. the frontend's dashboard), not a raw timestamp a user has to do
/// arithmetic on.
pub fn last_sync_line(status: &AgentStatus, now: DateTime<Utc>) -> String {
    let Some(last_sync) = status.last_sync else {
        return "Последняя синхронизация: ещё не было".to_string();
    };
    let elapsed = now.signed_duration_since(last_sync);
    // `< 1` minute covers both a genuinely recent sync AND a negative
    // elapsed duration from momentary clock skew (agent's clock briefly
    // ahead of ours) — both collapse to the same "только что" label rather
    // than ever rendering a negative number.
    let label = if elapsed.num_minutes() < 1 {
        "только что".to_string()
    } else if elapsed.num_minutes() < 60 {
        format!("{} мин. назад", elapsed.num_minutes())
    } else if elapsed.num_hours() < 24 {
        format!("{} ч. назад", elapsed.num_hours())
    } else {
        format!("{} дн. назад", elapsed.num_days())
    };
    format!("Последняя синхронизация: {label}")
}

pub fn pending_line(status: &AgentStatus) -> String {
    match status.pending_count {
        0 => "Ожидают отправки: нет".to_string(),
        n => format!("Ожидают отправки: {n}"),
    }
}

pub fn pause_resume_label(status: &AgentStatus) -> &'static str {
    if status.is_paused {
        "Возобновить"
    } else {
        "Приостановить"
    }
}

pub fn version_line(status: &AgentStatus) -> String {
    format!("Версия {}", status.agent_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        paired: bool,
        is_paused: bool,
        last_sync: Option<DateTime<Utc>>,
        pending_count: usize,
    ) -> AgentStatus {
        AgentStatus {
            paired,
            is_paused,
            last_sync,
            pending_count,
            agent_version: "0.1.0-rust".to_string(),
            available_update_version: None,
        }
    }

    #[test]
    fn status_line_covers_all_three_meaningful_states() {
        assert_eq!(status_line(&status(false, false, None, 0)), "Не привязано");
        assert_eq!(status_line(&status(false, true, None, 0)), "Не привязано"); // unpaired wins regardless of pause flag — pausing an unpaired agent isn't a meaningful state to surface separately
        assert_eq!(status_line(&status(true, true, None, 0)), "Приостановлено");
        assert_eq!(status_line(&status(true, false, None, 0)), "Работает");
    }

    #[test]
    fn tooltip_line_covers_running_paused_unpaired_and_update_available() {
        assert_eq!(tooltip_line(&status(true, false, None, 0)), "DevPace — работает");
        assert_eq!(tooltip_line(&status(true, true, None, 0)), "DevPace — остановлен");
        assert_eq!(tooltip_line(&status(false, false, None, 0)), "DevPace — не привязано");

        let mut with_update = status(true, false, None, 0);
        with_update.available_update_version = Some("0.1.99".to_string());
        assert_eq!(tooltip_line(&with_update), "DevPace — есть обновление");

        // An available update is worth surfacing even while paused/unpaired
        // — it wins over both.
        let mut paused_with_update = status(true, true, None, 0);
        paused_with_update.available_update_version = Some("0.1.99".to_string());
        assert_eq!(tooltip_line(&paused_with_update), "DevPace — есть обновление");
    }

    #[test]
    fn last_sync_line_handles_never_synced() {
        let s = status(true, false, None, 0);
        assert_eq!(
            last_sync_line(&s, Utc::now()),
            "Последняя синхронизация: ещё не было"
        );
    }

    #[test]
    fn last_sync_line_uses_coarsest_appropriate_unit() {
        let now: DateTime<Utc> = "2026-07-18T12:00:00Z".parse().unwrap();
        let cases = [
            (now - chrono::Duration::seconds(10), "только что"),
            (now - chrono::Duration::minutes(5), "5 мин. назад"),
            (now - chrono::Duration::hours(3), "3 ч. назад"),
            (now - chrono::Duration::days(2), "2 дн. назад"),
        ];
        for (last_sync, expected_fragment) in cases {
            let s = status(true, false, Some(last_sync), 0);
            let line = last_sync_line(&s, now);
            assert!(
                line.contains(expected_fragment),
                "line {line:?} did not contain {expected_fragment:?}"
            );
        }
    }

    #[test]
    fn last_sync_line_never_shows_a_negative_duration_under_clock_skew() {
        let now: DateTime<Utc> = "2026-07-18T12:00:00Z".parse().unwrap();
        let future_sync = now + chrono::Duration::seconds(5); // agent's clock briefly ahead of ours
        let s = status(true, false, Some(future_sync), 0);
        let line = last_sync_line(&s, now);
        assert!(
            !line.contains('-'),
            "must never render a negative elapsed duration: {line:?}"
        );
    }

    #[test]
    fn pending_line_distinguishes_zero_from_nonzero() {
        assert_eq!(
            pending_line(&status(true, false, None, 0)),
            "Ожидают отправки: нет"
        );
        assert_eq!(
            pending_line(&status(true, false, None, 7)),
            "Ожидают отправки: 7"
        );
    }

    #[test]
    fn pause_resume_label_toggles() {
        assert_eq!(
            pause_resume_label(&status(true, false, None, 0)),
            "Приостановить"
        );
        assert_eq!(
            pause_resume_label(&status(true, true, None, 0)),
            "Возобновить"
        );
    }
}
