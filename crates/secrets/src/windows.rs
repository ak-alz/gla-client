//! Real Windows Credential Manager backend — `CredWriteW`/
//! `CredReadW`/`CredDeleteW` (generic credential type), the same OS
//! facility the Windows Credential Manager Control Panel applet
//! displays. Persisted `CRED_PERSIST_LOCAL_MACHINE` — survives reboots,
//! scoped to the current Windows user account by the OS itself (no
//! separate ACL step needed here — this is what "OS secure storage"
//! means on Windows, distinct from `permissions::restrict_to_current_user_only`,
//! which hardens plain files elsewhere in the data directory, not
//! credentials that never touch the filesystem as plaintext at all).

use crate::secret_string::SecretString;
use crate::store::SecretStore;
use std::fmt;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

#[derive(Debug)]
pub struct WindowsCredentialError {
    operation: &'static str,
    win32_error: u32,
}

impl fmt::Display for WindowsCredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Windows Credential Manager {} failed (Win32 error {})",
            self.operation, self.win32_error
        )
    }
}

impl std::error::Error for WindowsCredentialError {}

pub struct WindowsCredentialStore {
    /// The credential's "target name" — the identifier Credential
    /// Manager lists it under. Scoped by product name so this never
    /// collides with an unrelated app's stored credential.
    target_name: Vec<u16>,
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl WindowsCredentialStore {
    pub fn new(target_name: &str) -> Self {
        Self {
            target_name: to_wide_null(target_name),
        }
    }
}

impl SecretStore for WindowsCredentialStore {
    type Error = WindowsCredentialError;

    fn store_token(&self, token: &SecretString) -> Result<(), Self::Error> {
        let blob = token.expose().as_bytes().to_vec();
        let mut target_name = self.target_name.clone();

        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target_name.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: unsafe { std::mem::zeroed() },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: std::ptr::null_mut(),
        };

        let ok = unsafe { CredWriteW(&credential, 0) };
        if ok == 0 {
            return Err(WindowsCredentialError {
                operation: "write",
                win32_error: unsafe { GetLastError() },
            });
        }
        Ok(())
    }

    fn load_token(&self) -> Result<Option<SecretString>, Self::Error> {
        let mut target_name = self.target_name.clone();
        let mut credential_ptr: *mut CREDENTIALW = std::ptr::null_mut();

        let ok = unsafe {
            CredReadW(
                target_name.as_mut_ptr(),
                CRED_TYPE_GENERIC,
                0,
                &mut credential_ptr,
            )
        };

        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(WindowsCredentialError {
                operation: "read",
                win32_error: err,
            });
        }

        let result = unsafe {
            let cred = &*credential_ptr;
            let blob =
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize);
            String::from_utf8(blob.to_vec())
        };

        unsafe { CredFree(credential_ptr as *const _) };

        match result {
            Ok(token) => Ok(Some(SecretString::new(token))),
            Err(_) => Err(WindowsCredentialError {
                operation: "read (blob was not valid UTF-8)",
                win32_error: 0,
            }),
        }
    }

    fn revoke(&self) -> Result<(), Self::Error> {
        let mut target_name = self.target_name.clone();
        let ok = unsafe { CredDeleteW(target_name.as_mut_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(()); // already absent — idempotent, not an error
            }
            return Err(WindowsCredentialError {
                operation: "delete",
                win32_error: err,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> WindowsCredentialStore {
        WindowsCredentialStore::new(&format!(
            "growth-layer-agent-secrets-test-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn store_then_load_round_trips_the_real_value() {
        let store = test_store("roundtrip");
        store
            .store_token(&SecretString::new("real-token-value-123"))
            .unwrap();

        let loaded = store.load_token().unwrap();
        assert_eq!(
            loaded.map(|s| s.expose().to_string()),
            Some("real-token-value-123".to_string())
        );

        store.revoke().unwrap();
    }

    #[test]
    fn loading_before_ever_storing_returns_none_not_an_error() {
        let store = test_store("never-stored");
        assert!(store.load_token().unwrap().is_none());
    }

    #[test]
    fn revoke_removes_the_stored_token() {
        let store = test_store("revoke");
        store
            .store_token(&SecretString::new("to-be-revoked"))
            .unwrap();
        assert!(store.load_token().unwrap().is_some());

        store.revoke().unwrap();
        assert!(store.load_token().unwrap().is_none());
    }

    #[test]
    fn revoke_without_anything_stored_is_a_safe_noop() {
        let store = test_store("revoke-noop");
        assert!(store.revoke().is_ok());
    }

    #[test]
    fn storing_again_overwrites_the_previous_value() {
        let store = test_store("overwrite");
        store
            .store_token(&SecretString::new("first-value"))
            .unwrap();
        store
            .store_token(&SecretString::new("second-value"))
            .unwrap();

        let loaded = store.load_token().unwrap();
        assert_eq!(
            loaded.map(|s| s.expose().to_string()),
            Some("second-value".to_string())
        );

        store.revoke().unwrap();
    }
}
