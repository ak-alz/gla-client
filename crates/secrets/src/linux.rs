//! Real Linux Secret Service backend — talks to whatever provider is
//! registered on the session D-Bus (`gnome-keyring-daemon`, `kwalletd`,
//! or any other implementation of the freedesktop.org Secret Service
//! spec), via the `secret-service` crate's blocking API (itself built
//! on `zbus`, the same D-Bus library `linux-collector`'s `native_loop.rs`
//! already uses for real `org.freedesktop.login1` signals).

use crate::secret_string::SecretString;
use crate::store::SecretStore;
use secret_service::blocking::SecretService;
use secret_service::EncryptionType;
use std::collections::HashMap;

const ATTRIBUTE_KEY: &str = "service";

#[derive(Debug, thiserror::Error)]
pub enum SecretServiceError {
    #[error("secret service error: {0}")]
    Backend(#[from] secret_service::Error),
    #[error("stored secret was not valid UTF-8 (data corruption or a non-agent item sharing this attribute)")]
    NotUtf8,
}

pub struct LinuxSecretStore {
    /// Distinguishes this app's credential from anything else stored
    /// in the same keyring/collection — the `service` attribute value
    /// every item this store creates/searches for carries.
    service_name: String,
}

impl LinuxSecretStore {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    fn attributes(&self) -> HashMap<&str, &str> {
        let mut attrs = HashMap::new();
        attrs.insert(ATTRIBUTE_KEY, self.service_name.as_str());
        attrs
    }
}

impl SecretStore for LinuxSecretStore {
    type Error = SecretServiceError;

    fn store_token(&self, token: &SecretString) -> Result<(), Self::Error> {
        let ss = SecretService::connect(EncryptionType::Dh)?;
        let collection = ss.get_default_collection()?;
        if collection.is_locked()? {
            collection.unlock()?;
        }
        collection.create_item(
            "Growth Layer Agent device token",
            self.attributes(),
            token.expose().as_bytes(),
            true, // replace any existing item with the same attributes
            "text/plain",
        )?;
        Ok(())
    }

    fn load_token(&self) -> Result<Option<SecretString>, Self::Error> {
        let ss = SecretService::connect(EncryptionType::Dh)?;
        let items = ss.search_items(self.attributes())?;
        // `unlocked` items first (no prompt needed); fall back to a
        // `locked` one and unlock it explicitly — either way, at most
        // one item should ever match this store's attributes (see
        // `store_token`'s `replace: true`).
        let item = match items.unlocked.into_iter().next() {
            Some(item) => item,
            None => match items.locked.into_iter().next() {
                Some(item) => {
                    item.unlock()?;
                    item
                }
                None => return Ok(None),
            },
        };

        let secret_bytes = item.get_secret()?;
        let token = String::from_utf8(secret_bytes).map_err(|_| SecretServiceError::NotUtf8)?;
        Ok(Some(SecretString::new(token)))
    }

    fn revoke(&self) -> Result<(), Self::Error> {
        let ss = SecretService::connect(EncryptionType::Dh)?;
        let items = ss.search_items(self.attributes())?;
        for item in items.unlocked.into_iter().chain(items.locked) {
            item.delete()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> LinuxSecretStore {
        LinuxSecretStore::new(format!(
            "growth-layer-agent-secrets-test-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn store_then_load_round_trips_the_real_value_against_a_real_secret_service() {
        let store = test_store("roundtrip");
        store
            .store_token(&SecretString::new("real-token-value-123"))
            .expect("store must succeed against a real Secret Service provider");

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
