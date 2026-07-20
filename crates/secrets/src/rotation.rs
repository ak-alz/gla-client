//! Pure token-rotation decision logic — no OS/network dependency, same
//! "pure state machine, thin OS-touching layer elsewhere" split
//! `lifecycle::power_events`/`updater::restart_policy` already
//! established. This crate does not itself CALL a backend to obtain a
//! new token (that's a backend/pairing-flow concern outside
//! `agent-core`'s scope, same boundary already drawn for signing in
//! `update-manifest`) — it only decides WHETHER the currently stored
//! token is due for rotation.

use crate::secret_string::SecretString;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub token: SecretString,
    pub issued_at: DateTime<Utc>,
    /// `None` = a token with no known expiry (e.g. a manually-issued
    /// long-lived token) — never flagged for rotation on that basis
    /// alone; only an actual approaching expiry triggers rotation.
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy)]
pub struct RotationPolicy {
    /// Rotate once fewer than this much time remains before
    /// `expires_at` — gives the rotation flow a real window to
    /// complete before the old token actually stops working, rather
    /// than reacting only after it's already too late (a 401 from the
    /// backend).
    pub rotate_before_expiry: Duration,
}

impl Default for RotationPolicy {
    fn default() -> Self {
        Self {
            rotate_before_expiry: Duration::days(7),
        }
    }
}

impl RotationPolicy {
    pub fn needs_rotation(&self, record: &TokenRecord, now: DateTime<Utc>) -> bool {
        match record.expires_at {
            None => false,
            Some(expires_at) => expires_at - now <= self.rotate_before_expiry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_expiring_in(days: i64) -> TokenRecord {
        TokenRecord {
            token: SecretString::new("token"),
            issued_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::days(days)),
        }
    }

    #[test]
    fn a_token_with_no_expiry_never_needs_rotation() {
        let policy = RotationPolicy::default();
        let record = TokenRecord {
            token: SecretString::new("token"),
            issued_at: Utc::now(),
            expires_at: None,
        };
        assert!(!policy.needs_rotation(&record, Utc::now()));
    }

    #[test]
    fn a_token_expiring_well_outside_the_window_does_not_need_rotation() {
        let policy = RotationPolicy::default();
        let record = record_expiring_in(30);
        assert!(!policy.needs_rotation(&record, Utc::now()));
    }

    #[test]
    fn a_token_expiring_within_the_window_needs_rotation() {
        let policy = RotationPolicy::default();
        let record = record_expiring_in(3);
        assert!(policy.needs_rotation(&record, Utc::now()));
    }

    #[test]
    fn an_already_expired_token_needs_rotation() {
        let policy = RotationPolicy::default();
        let record = record_expiring_in(-1);
        assert!(policy.needs_rotation(&record, Utc::now()));
    }

    #[test]
    fn the_exact_boundary_counts_as_needing_rotation() {
        let policy = RotationPolicy {
            rotate_before_expiry: Duration::days(7),
        };
        let record = TokenRecord {
            token: SecretString::new("token"),
            issued_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::days(7)),
        };
        assert!(policy.needs_rotation(&record, Utc::now()));
    }
}
