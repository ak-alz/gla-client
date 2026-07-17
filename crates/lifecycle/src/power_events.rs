//! Pure state machine for sleep/wake, session lock/unlock, and logoff —
//! no OS dependency, fully unit-testable. The actual OS event REGISTRATION
//! (hooking `WM_POWERBROADCAST`/`WM_WTSSESSION_CHANGE`/`WM_QUERYENDSESSION`
//! on Windows) is a separate, thin, unavoidably-untestable-in-CI layer —
//! see TEST_REPORT.md for why the "sleep/wake проходит 20 cycles" and
//! "lock/unlock корректен" criteria are verified here via 20+ SIMULATED
//! cycles, not by actually suspending or locking the machine this session
//! runs on (which would be disruptive to an interactive user session, not
//! just inconvenient to automate).

/// One OS-level lifecycle notification this crate reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    Resume,
    SessionLock,
    SessionUnlock,
    /// The OS is asking whether it's safe to end the session (logoff/
    /// shutdown/restart) — still cancelable in principle, but this crate
    /// always treats it as "start winding down," matching the acceptance
    /// criterion that quitting must be prompt, not that it must contest
    /// the session ending.
    QueryEndSession,
    /// The session is ending, unconditionally.
    EndSession,
}

/// What the caller (the future top-level agent binary) should do in
/// response to an event. This crate does not itself pause a collector or
/// flush a queue — it only decides WHEN that should happen; the caller
/// wires the actual pause/resume/flush calls (matching the same
/// caller-supplies-the-real-effects pattern as `uploader::CycleOutcome`
/// and `ui-shell::AgentController`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    /// Nothing needs to change — e.g. a redundant Suspend while already
    /// suspended (some OS event pairs aren't perfectly balanced).
    Continue,
    /// Stop actively polling/uploading — the system is suspending or the
    /// session just locked. Resuming/uploading further work right now
    /// would burn battery/cycles on state that's about to go stale or be
    /// inaccessible anyway.
    PauseWork,
    /// Safe to resume normal polling/uploading.
    ResumeWork,
    /// The process is being asked to end — flush and exit promptly, do
    /// not start new long-running work.
    ///
    /// `LifecycleState` does not remember that this was already emitted —
    /// it is a one-way real-world event ("the session is ending"), not a
    /// state this pure struct tracks, so calling `handle()` again
    /// afterward (e.g. a stray `Suspend`/`Resume` pair arriving after
    /// `EndSession`) would still emit `ResumeWork`/`PauseWork` as if
    /// nothing had happened. The caller owns treating `PrepareToExit` as
    /// a point of no return and must stop calling `handle()` (or ignore
    /// its output) once it's seen one — an independent review flagged
    /// this explicitly as an intentional scope boundary, not an
    /// oversight, so it's spelled out here for whichever future task
    /// wires this into a real running agent.
    PrepareToExit,
}

/// Tracks suspended/locked as independent flags (not a single enum)
/// because they can overlap in practice (a machine can be locked AND then
/// go to sleep while locked) — `is_active()` is the AND of "not suspended"
/// and "not locked," so resuming from sleep while still locked correctly
/// stays paused, and unlocking while still suspended (unusual, but not
/// impossible depending on OS event ordering) correctly stays paused too.
#[derive(Debug, Default)]
pub struct LifecycleState {
    suspended: bool,
    locked: bool,
    suspend_count: u64,
    lock_count: u64,
}

impl LifecycleState {
    pub fn new() -> Self {
        Self::default()
    }

    fn is_active(&self) -> bool {
        !self.suspended && !self.locked
    }

    /// Applies one event and returns what the caller should now do. Pure
    /// and deterministic — the same starting state plus the same event
    /// always produces the same resulting state and action.
    pub fn handle(&mut self, event: PowerEvent) -> LifecycleAction {
        match event {
            PowerEvent::Suspend => {
                let was_active = self.is_active();
                if !self.suspended {
                    self.suspend_count += 1;
                }
                self.suspended = true;
                if was_active {
                    LifecycleAction::PauseWork
                } else {
                    LifecycleAction::Continue
                }
            }
            PowerEvent::Resume => {
                self.suspended = false;
                if self.is_active() {
                    LifecycleAction::ResumeWork
                } else {
                    LifecycleAction::Continue
                }
            }
            PowerEvent::SessionLock => {
                let was_active = self.is_active();
                if !self.locked {
                    self.lock_count += 1;
                }
                self.locked = true;
                if was_active {
                    LifecycleAction::PauseWork
                } else {
                    LifecycleAction::Continue
                }
            }
            PowerEvent::SessionUnlock => {
                self.locked = false;
                if self.is_active() {
                    LifecycleAction::ResumeWork
                } else {
                    LifecycleAction::Continue
                }
            }
            PowerEvent::QueryEndSession | PowerEvent::EndSession => LifecycleAction::PrepareToExit,
        }
    }

    pub fn suspend_count(&self) -> u64 {
        self.suspend_count
    }

    pub fn lock_count(&self) -> u64 {
        self.lock_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_then_resume_pauses_then_resumes() {
        let mut state = LifecycleState::new();
        assert_eq!(
            state.handle(PowerEvent::Suspend),
            LifecycleAction::PauseWork
        );
        assert_eq!(
            state.handle(PowerEvent::Resume),
            LifecycleAction::ResumeWork
        );
        assert_eq!(state.suspend_count(), 1);
    }

    #[test]
    fn lock_then_unlock_pauses_then_resumes() {
        let mut state = LifecycleState::new();
        assert_eq!(
            state.handle(PowerEvent::SessionLock),
            LifecycleAction::PauseWork
        );
        assert_eq!(
            state.handle(PowerEvent::SessionUnlock),
            LifecycleAction::ResumeWork
        );
        assert_eq!(state.lock_count(), 1);
    }

    #[test]
    fn twenty_suspend_resume_cycles_stay_consistent() {
        // Satisfies "Sleep/wake проходит 20 cycles" via simulation — see
        // module doc comment for why a real 20x machine-sleep isn't run.
        let mut state = LifecycleState::new();
        for cycle in 1..=20u64 {
            assert_eq!(
                state.handle(PowerEvent::Suspend),
                LifecycleAction::PauseWork,
                "cycle {cycle}: suspend must pause from an active state every time"
            );
            assert_eq!(
                state.handle(PowerEvent::Resume),
                LifecycleAction::ResumeWork,
                "cycle {cycle}: resume must resume when nothing else is holding it paused"
            );
        }
        assert_eq!(state.suspend_count(), 20);
    }

    #[test]
    fn locking_while_suspended_does_not_prematurely_resume_on_unlock() {
        // The overlap case the two-independent-flags design exists for:
        // suspend, then (still suspended) lock, then unlock — must NOT
        // resume work while still suspended; only resuming from suspend
        // afterward should actually resume it.
        let mut state = LifecycleState::new();
        assert_eq!(
            state.handle(PowerEvent::Suspend),
            LifecycleAction::PauseWork
        );
        assert_eq!(
            state.handle(PowerEvent::SessionLock),
            LifecycleAction::Continue,
            "already paused by suspend — locking too must not double-signal"
        );
        assert_eq!(
            state.handle(PowerEvent::SessionUnlock),
            LifecycleAction::Continue,
            "must not resume while still suspended, even though the lock that was also holding it paused just cleared"
        );
        assert_eq!(
            state.handle(PowerEvent::Resume),
            LifecycleAction::ResumeWork,
            "now both conditions are clear — safe to resume"
        );
    }

    #[test]
    fn redundant_events_are_idempotent_and_do_not_double_count() {
        let mut state = LifecycleState::new();
        assert_eq!(
            state.handle(PowerEvent::Suspend),
            LifecycleAction::PauseWork
        );
        assert_eq!(
            state.handle(PowerEvent::Suspend),
            LifecycleAction::Continue,
            "a second Suspend with no Resume in between must not re-signal or double-count"
        );
        assert_eq!(state.suspend_count(), 1);
    }

    #[test]
    fn query_end_session_and_end_session_always_prepare_to_exit() {
        for initial in [PowerEvent::Suspend, PowerEvent::SessionLock] {
            let mut state = LifecycleState::new();
            state.handle(initial);
            assert_eq!(
                state.handle(PowerEvent::QueryEndSession),
                LifecycleAction::PrepareToExit
            );
            assert_eq!(
                state.handle(PowerEvent::EndSession),
                LifecycleAction::PrepareToExit
            );
        }
    }
}
