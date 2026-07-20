//! "Secrets absent from logs" — a wrapper that makes it structurally
//! hard to accidentally log a token, rather than relying on every call
//! site remembering not to. `Debug`/`Display` both redact; the only way
//! to get the real value out is the explicitly-named `expose()`, a
//! deliberate, visible-in-a-diff call site any reviewer would notice.
//! Also zeroizes its backing memory on drop (defense in depth against
//! a memory scrape/core dump reading a freed-but-not-cleared value) —
//! note this only clears THIS instance's memory, not any `.clone()`s
//! made from it, the same documented limitation the `zeroize` crate
//! itself carries for any owned, clonable type.

use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// `#[serde(transparent)]` — serializes/deserializes exactly as a bare
/// string (matching, e.g., an existing `config.json`'s
/// `"agent_token": "..."` field unchanged) — this type's protection is
/// against accidental `Debug`/`Display`-based logging, not against its
/// necessary, explicit, designated storage format.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read the real value — named to be impossible to
    /// call by accident (`.expose()`, not `.value()`/`.get()`/`.0`).
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(REDACTED)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("REDACTED")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_formatting_never_reveals_the_real_value() {
        let secret = SecretString::new("super-secret-token-abc123");
        let formatted = format!("{secret:?}");
        assert!(!formatted.contains("super-secret-token-abc123"));
        assert_eq!(formatted, "SecretString(REDACTED)");
    }

    #[test]
    fn display_formatting_never_reveals_the_real_value() {
        let secret = SecretString::new("super-secret-token-abc123");
        let formatted = format!("{secret}");
        assert!(!formatted.contains("super-secret-token-abc123"));
    }

    #[test]
    fn expose_returns_the_real_value_verbatim() {
        let secret = SecretString::new("super-secret-token-abc123");
        assert_eq!(secret.expose(), "super-secret-token-abc123");
    }

    /// The serialized form is DELIBERATELY the plain value (not
    /// redacted) — this type's protection is against accidental
    /// `Debug`/`Display` logging, not against its necessary,
    /// designated storage format (e.g. a config file's `agent_token`
    /// field on disk). Verified explicitly so the two are never
    /// confused with each other.
    #[test]
    fn serialization_carries_the_real_value_transparently_as_a_bare_string() {
        let secret = SecretString::new("super-secret-token-abc123");
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, "\"super-secret-token-abc123\"");

        let parsed: SecretString = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.expose(), "super-secret-token-abc123");
    }

    /// Reads the backing buffer's bytes DIRECTLY (via a pointer
    /// captured while the `String` is still validly allocated, before
    /// any drop/deallocation happens) to prove `zeroize()` actually
    /// overwrites the real bytes — not just that `Drop` compiles and
    /// runs without panicking. Reading memory that's already been
    /// freed would be real undefined behavior; this test never does
    /// that — the pointer is only ever read while `secret.0` (and
    /// therefore its allocation) is still alive and in scope.
    #[test]
    fn zeroize_actually_overwrites_the_backing_bytes() {
        let mut secret = SecretString::new("super-secret-token-abc123");
        let ptr = secret.0.as_ptr();
        let original_len = secret.0.len();
        assert!(original_len > 0);

        secret.0.zeroize();

        let bytes_after = unsafe { std::slice::from_raw_parts(ptr, original_len) };
        assert!(
            bytes_after.iter().all(|&b| b == 0),
            "expected every original byte to be zeroed, got {bytes_after:?}"
        );
    }

    #[test]
    fn formatting_inside_a_larger_debug_struct_still_redacts() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Wrapper {
            token: SecretString,
        }
        let w = Wrapper {
            token: SecretString::new("super-secret-token-abc123"),
        };
        let formatted = format!("{w:?}");
        assert!(!formatted.contains("super-secret-token-abc123"));
    }
}
