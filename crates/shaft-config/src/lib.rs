//! Shaft configuration: data model, on-disk persistence, Keychain-backed
//! secrets, upstream/policy validation and `ngrok.yml` interop.

pub mod model;
pub mod ngrok_yml;
pub mod secrets;
pub mod store;
pub mod upstream;

pub use model::*;
#[cfg(target_os = "macos")]
pub use secrets::KeychainSecretStore;
pub use secrets::{MemorySecretStore, SecretError, SecretStore};
pub use store::{app_support_dir, default_ngrok_yml, log_dir, ConfigStore, BUNDLE_ID};
pub use upstream::{normalize_upstream, probe_addr, traffic_policy_to_json, validate_profile};
