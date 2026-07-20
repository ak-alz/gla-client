//! The platform-agnostic contract every OS secure-storage backend
//! implements — same shape as `collector_core::SignalCollector`: one
//! shared trait, one real implementation per platform, `Error` as an
//! associated type (each OS's failure modes are genuinely different).

use crate::secret_string::SecretString;

pub trait SecretStore {
    type Error: std::error::Error;

    /// Persists `token` in OS secure storage, replacing any
    /// previously stored value for this service/account pair.
    fn store_token(&self, token: &SecretString) -> Result<(), Self::Error>;

    /// `Ok(None)` if nothing has been stored yet (or it was revoked) —
    /// a normal, expected state, not an error.
    fn load_token(&self) -> Result<Option<SecretString>, Self::Error>;

    /// "Device revoke" — deletes the stored token outright. After
    /// this, `load_token()` returns `Ok(None)` until `store_token` is
    /// called again (e.g. via a fresh pairing flow). Idempotent:
    /// revoking when nothing is stored is a safe no-op, matching this
    /// project's established "absence is already the desired state"
    /// convention (`lifecycle::CrashMarker::mark_clean_exit`,
    /// `updater::Staging::commit`).
    fn revoke(&self) -> Result<(), Self::Error>;
}
