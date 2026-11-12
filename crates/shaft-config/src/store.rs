//! Loading and saving [`AppConfig`] to disk, moving secrets to/from the
//! [`SecretStore`].

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::{
    model::{AppConfig, Secret, TunnelProfile, SCHEMA_VERSION},
    secrets::SecretStore,
};

pub const BUNDLE_ID: &str = "dev.shaft.app";

/// `~/Library/Application Support/dev.shaft.app`
pub fn app_support_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join(BUNDLE_ID)
}

/// `~/Library/Logs/dev.shaft.app`
pub fn log_dir() -> PathBuf {
    dirs::home_dir().map(|h| h.join("Library/Logs").join(BUNDLE_ID)).unwrap_or_else(|| app_support_dir().join("logs"))
}

/// Default location of the ngrok agent config on macOS.
pub fn default_ngrok_yml() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("ngrok/ngrok.yml")
}

fn account_key(id: Uuid, field: &str) -> String {
    format!("account/{id}/{field}")
}

fn profile_key(id: Uuid, field: &str) -> String {
    format!("profile/{id}/{field}")
}

pub struct ConfigStore {
    path: PathBuf,
    secrets: Arc<dyn SecretStore>,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { path: path.into(), secrets }
    }

    /// Store at the default location (`config.toml` in the app support dir).
    pub fn default_location(secrets: Arc<dyn SecretStore>) -> Self {
        Self::new(app_support_dir().join("config.toml"), secrets)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn secrets(&self) -> &Arc<dyn SecretStore> {
        &self.secrets
    }

    /// Load the config (default if the file does not exist) and hydrate secrets.
    pub fn load(&self) -> Result<AppConfig> {
        let mut cfg = if self.path.exists() {
            let text = fs::read_to_string(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
            let value: toml::Value = toml::from_str(&text).with_context(|| format!("parsing {}", self.path.display()))?;
            let migrated = migrate(value)?;
            migrated.try_into().context("decoding config")?
        } else {
            AppConfig::default()
        };
        self.hydrate(&mut cfg);
        Ok(cfg)
    }

    fn hydrate(&self, cfg: &mut AppConfig) {
        for acc in &mut cfg.accounts {
            acc.authtoken = self.read_secret(&account_key(acc.id, "authtoken"));
            acc.api_key = self.read_secret(&account_key(acc.id, "api_key"));
        }
        for p in &mut cfg.profiles {
            let id = p.id;
            for (field, slot) in p.secrets_mut() {
                *slot = self.read_secret(&profile_key(id, &field));
            }
        }
    }

    fn read_secret(&self, key: &str) -> Secret {
        match self.secrets.get(key) {
            Ok(v) => Secret::new(v.unwrap_or_default()),
            Err(e) => {
                tracing::warn!(key, error = %e, "could not read secret");
                Secret::default()
            }
        }
    }

    fn write_secret(&self, key: &str, value: &Secret) -> Result<()> {
        if value.is_empty() {
            self.secrets.delete(key)?;
        } else {
            self.secrets.set(key, value.expose())?;
        }
        Ok(())
    }

    /// Persist secrets for every account/profile and write the file atomically.
    pub fn save(&self, cfg: &AppConfig) -> Result<()> {
        for acc in &cfg.accounts {
            self.write_secret(&account_key(acc.id, "authtoken"), &acc.authtoken)?;
            self.write_secret(&account_key(acc.id, "api_key"), &acc.api_key)?;
        }
        for p in &cfg.profiles {
            self.save_profile_secrets(p)?;
        }
        let mut cfg = cfg.clone();
        cfg.schema_version = SCHEMA_VERSION;
        let text = toml::to_string_pretty(&cfg).context("encoding config")?;
        write_atomic(&self.path, text.as_bytes())
    }

    fn save_profile_secrets(&self, p: &TunnelProfile) -> Result<()> {
        let mut p = p.clone();
        let id = p.id;
        // Clear stale basic-auth slots beyond the current count (up to a sane bound).
        let current: Vec<(String, Secret)> = p.secrets_mut().into_iter().map(|(k, s)| (k, s.clone())).collect();
        for i in 0..32 {
            let k = format!("basic_auth.{i}");
            if !current.iter().any(|(ck, _)| *ck == k) {
                self.secrets.delete(&profile_key(id, &k))?;
            }
        }
        for (field, value) in current {
            self.write_secret(&profile_key(id, &field), &value)?;
        }
        Ok(())
    }

    /// Remove every secret associated with an account.
    pub fn forget_account_secrets(&self, id: Uuid) -> Result<()> {
        self.secrets.delete(&account_key(id, "authtoken"))?;
        self.secrets.delete(&account_key(id, "api_key"))?;
        Ok(())
    }

    /// Remove every secret associated with a profile.
    pub fn forget_profile_secrets(&self, p: &TunnelProfile) -> Result<()> {
        for field in ["oauth.client_secret", "oidc.client_secret", "webhook.secret"] {
            self.secrets.delete(&profile_key(p.id, field))?;
        }
        for i in 0..32 {
            self.secrets.delete(&profile_key(p.id, &format!("basic_auth.{i}")))?;
        }
        Ok(())
    }
}

/// Upgrade an older on-disk document to the current schema.
pub(crate) fn migrate(mut value: toml::Value) -> Result<toml::Value> {
    let version = value.get("schema_version").and_then(|v| v.as_integer()).unwrap_or(SCHEMA_VERSION as i64);
    if version > SCHEMA_VERSION as i64 {
        anyhow::bail!("config schema {version} is newer than this version of Shaft supports ({SCHEMA_VERSION})");
    }
    // v1 is the first schema: nothing to migrate yet.
    if let Some(t) = value.as_table_mut() {
        t.insert("schema_version".into(), toml::Value::Integer(SCHEMA_VERSION as i64));
    }
    Ok(value)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("config path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::*, secrets::MemorySecretStore};

    #[test]
    fn roundtrip_with_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecretStore::new());
        let store = ConfigStore::new(dir.path().join("config.toml"), secrets.clone());

        let mut cfg = AppConfig::default();
        let mut acc = Account::new("me");
        acc.authtoken = "tok".into();
        let mut p = TunnelProfile::new("web", acc.id, Protocol::Http, "3000");
        if let ProtocolOptions::Http(h) = &mut p.options {
            h.basic_auth.push(BasicAuth { username: "u".into(), password: "pw".into() });
            h.oauth = Some(OAuth { provider: "google".into(), client_secret: "cs".into(), ..Default::default() });
        }
        cfg.accounts.push(acc);
        cfg.profiles.push(p);
        store.save(&cfg).unwrap();

        let text = fs::read_to_string(store.path()).unwrap();
        assert!(!text.contains("\"tok\""));
        assert!(!text.contains("\"pw\""));

        let back = store.load().unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.accounts[0].authtoken.expose(), "tok");

        // Removing the basic auth entry deletes its secret.
        let mut cfg2 = back.clone();
        if let ProtocolOptions::Http(h) = &mut cfg2.profiles[0].options {
            h.basic_auth.clear();
        }
        store.save(&cfg2).unwrap();
        let pid = cfg2.profiles[0].id;
        assert!(secrets.get(&format!("profile/{pid}/basic_auth.0")).unwrap().is_none());
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("nope.toml"), Arc::new(MemorySecretStore::new()));
        assert_eq!(store.load().unwrap(), AppConfig::default());
    }

    #[test]
    fn rejects_future_schema() {
        let v: toml::Value = toml::from_str("schema_version = 999").unwrap();
        assert!(migrate(v).is_err());
    }
}
