//! Pure decision logic for WHEN a staged update may actually restart
//! the process — no OS dependency, fully unit-testable, same "pure
//! state machine, thin OS-touching layer elsewhere" split
//! `lifecycle::power_events` already established. Directly implements
//! §10's rules: "Не перезапускать агент немедленно после загрузки,"
//! "Применять обновление при безопасном окне: logout, system restart,
//! явное подтверждение или длительный idle," "Security update может
//! быть обязательным."

#[derive(Debug, Clone, Copy)]
pub struct RestartContext {
    /// From `update_manifest::UnsignedManifest::mandatory` — "only for
    /// security emergency" per that crate's own doc comment. A
    /// mandatory update restarts regardless of the other fields below.
    pub mandatory: bool,
    /// The user clicked something like "restart now" — an explicit,
    /// deliberate action, not inferred.
    pub user_explicitly_confirmed: bool,
    /// The OS session itself is ending (logout/shutdown/restart) —
    /// restarting into the new version now doesn't interrupt anything
    /// that wasn't already ending.
    pub session_ending: bool,
    pub idle_seconds: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    /// How long the user must have been idle before "long idle" counts
    /// as a safe window on its own.
    pub long_idle_threshold_seconds: f64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            long_idle_threshold_seconds: 600.0, // 10 minutes
        }
    }
}

impl RestartPolicy {
    /// `true` = safe to restart into the staged update right now.
    /// `false` = defer — keep running the current version; the update
    /// stays staged (not yet swapped in) until a later check returns
    /// `true`. This is exactly what "user can postpone non-security
    /// restart" means in practice: nothing forces the restart, this
    /// function is simply asked again later (e.g. next idle-check
    /// tick, next confirmation, next logout).
    pub fn should_restart_now(&self, ctx: &RestartContext) -> bool {
        ctx.mandatory
            || ctx.user_explicitly_confirmed
            || ctx.session_ending
            || ctx.idle_seconds >= self.long_idle_threshold_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_ctx() -> RestartContext {
        RestartContext {
            mandatory: false,
            user_explicitly_confirmed: false,
            session_ending: false,
            idle_seconds: 0.0,
        }
    }

    #[test]
    fn a_non_mandatory_update_with_no_safe_window_is_deferred() {
        let policy = RestartPolicy::default();
        assert!(!policy.should_restart_now(&base_ctx()));
    }

    #[test]
    fn a_mandatory_update_restarts_regardless_of_everything_else() {
        let policy = RestartPolicy::default();
        let ctx = RestartContext {
            mandatory: true,
            ..base_ctx()
        };
        assert!(policy.should_restart_now(&ctx));
    }

    #[test]
    fn explicit_user_confirmation_allows_restart() {
        let policy = RestartPolicy::default();
        let ctx = RestartContext {
            user_explicitly_confirmed: true,
            ..base_ctx()
        };
        assert!(policy.should_restart_now(&ctx));
    }

    #[test]
    fn a_session_ending_is_treated_as_a_safe_window() {
        let policy = RestartPolicy::default();
        let ctx = RestartContext {
            session_ending: true,
            ..base_ctx()
        };
        assert!(policy.should_restart_now(&ctx));
    }

    #[test]
    fn long_idle_past_the_threshold_allows_restart_short_idle_does_not() {
        let policy = RestartPolicy::default();
        let short_idle = RestartContext {
            idle_seconds: 599.0,
            ..base_ctx()
        };
        let long_idle = RestartContext {
            idle_seconds: 600.0,
            ..base_ctx()
        };
        assert!(!policy.should_restart_now(&short_idle));
        assert!(policy.should_restart_now(&long_idle));
    }
}
