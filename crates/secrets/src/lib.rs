//! `AG-SEC-001` — OS secure storage for the device auth token, token
//! rotation policy, device revoke, log-safe `SecretString`, and
//! file-permission hardening. See
//! `docs/02_ARCHITECTURE/AGENT_THREAT_MODEL.md` for the full threat
//! model this crate is one part of.
//!
//! Real backends: Windows Credential Manager (`windows.rs`,
//! `CredWriteW`/`CredReadW`/`CredDeleteW`), Linux Secret Service
//! (`linux.rs`, the real freedesktop.org D-Bus API — gnome-keyring/
//! kwallet). macOS Keychain is NOT implemented here — consistent with
//! `AG-MAC-001`'s honest "no macOS hardware, no guessed FFI code"
//! stance, a real Keychain backend needs the same real-hardware
//! verification that crate's `todo!()` stubs are waiting for.

pub mod permissions;
pub mod rotation;
pub mod secret_string;
pub mod store;

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows::{WindowsCredentialError, WindowsCredentialStore};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::{LinuxSecretStore, SecretServiceError};

pub use permissions::{restrict_to_current_user_only, PermissionError};
pub use rotation::{RotationPolicy, TokenRecord};
pub use secret_string::SecretString;
pub use store::SecretStore;
