//! Secret storage. On macOS secrets live in the login Keychain as generic
//! passwords under the service [`KEYCHAIN_SERVICE`]; tests use [`MemorySecretStore`].

use std::collections::HashMap;

use parking_lot::Mutex;

pub const KEYCHAIN_SERVICE: &str = "dev.shaft.app";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("keychain error: {0}")]
    Backend(String),
}

pub trait SecretStore: Send + Sync {
    /// Returns `Ok(None)` when the item does not exist.
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;
    /// Deleting a missing item is not an error.
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// In-memory store for tests and for running without a Keychain.
#[derive(Default)]
pub struct MemorySecretStore {
    items: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.items.lock().len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.lock().is_empty()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self.items.lock().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.items.lock().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.items.lock().remove(key);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use keychain::KeychainSecretStore;

#[cfg(target_os = "macos")]
mod keychain {
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

    use super::*;

    /// errSecItemNotFound
    const NOT_FOUND: i32 = -25300;

    pub struct KeychainSecretStore {
        service: String,
    }

    impl KeychainSecretStore {
        pub fn new() -> Self {
            Self { service: KEYCHAIN_SERVICE.into() }
        }
        pub fn with_service(service: impl Into<String>) -> Self {
            Self { service: service.into() }
        }
    }

    impl Default for KeychainSecretStore {
        fn default() -> Self {
            Self::new()
        }
    }

    impl SecretStore for KeychainSecretStore {
        fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
            match get_generic_password(&self.service, key) {
                Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|e| SecretError::Backend(e.to_string())),
                Err(e) if e.code() == NOT_FOUND => Ok(None),
                Err(e) => Err(SecretError::Backend(e.to_string())),
            }
        }
        fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
            set_generic_password(&self.service, key, value.as_bytes()).map_err(|e| SecretError::Backend(e.to_string()))
        }
        fn delete(&self, key: &str) -> Result<(), SecretError> {
            match delete_generic_password(&self.service, key) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == NOT_FOUND => Ok(()),
                Err(e) => Err(SecretError::Backend(e.to_string())),
            }
        }
    }
}
