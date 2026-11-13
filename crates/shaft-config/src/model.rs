//! Persistent data model: settings, accounts and tunnel profiles.
//!
//! Secrets (authtokens, API keys, basic-auth passwords, OAuth/OIDC client
//! secrets, webhook secrets) are *never* serialized to disk: the fields are
//! marked `#[serde(skip)]` and moved in and out of the [`crate::SecretStore`]
//! by [`crate::ConfigStore`].

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current on-disk schema version. Bump and add a migration in
/// [`crate::store::migrate`] when the format changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub settings: Settings,
    pub accounts: Vec<Account>,
    pub profiles: Vec<TunnelProfile>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { schema_version: SCHEMA_VERSION, settings: Settings::default(), accounts: Vec::new(), profiles: Vec::new() }
    }
}

impl AppConfig {
    pub fn account(&self, id: Uuid) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }
    pub fn account_mut(&mut self, id: Uuid) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }
    pub fn profile(&self, id: Uuid) -> Option<&TunnelProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }
    pub fn profile_mut(&mut self, id: Uuid) -> Option<&mut TunnelProfile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }
    /// The account used for quick tunnels and API resources: the selected one,
    /// falling back to the first configured account.
    pub fn active_account(&self) -> Option<&Account> {
        self.settings.active_account.and_then(|id| self.account(id)).or_else(|| self.accounts.first())
    }
    /// Insert or replace (by id) an account.
    pub fn upsert_account(&mut self, account: Account) {
        match self.account_mut(account.id) {
            Some(existing) => *existing = account,
            None => self.accounts.push(account),
        }
    }
    /// Insert or replace (by id) a profile.
    pub fn upsert_profile(&mut self, profile: TunnelProfile) {
        match self.profile_mut(profile.id) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
    }
    /// Remove an account and every profile bound to it. Returns the removed profiles.
    pub fn remove_account(&mut self, id: Uuid) -> Vec<TunnelProfile> {
        self.accounts.retain(|a| a.id != id);
        if self.settings.active_account == Some(id) {
            self.settings.active_account = None;
        }
        let (removed, kept) = std::mem::take(&mut self.profiles).into_iter().partition(|p| p.account_id == id);
        self.profiles = kept;
        removed
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Copy the public URL to the clipboard when a tunnel comes online.
    pub copy_url_on_start: bool,
    pub notifications: NotificationSettings,
    pub hotkeys: HotkeySettings,
    pub inspector: InspectorSettings,
    pub reconnect: ReconnectSettings,
    pub updates: UpdateSettings,
    /// `tracing` filter directive, e.g. `info` or `shaft_core=debug`.
    pub log_level: String,
    pub active_account: Option<Uuid>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            copy_url_on_start: true,
            notifications: NotificationSettings::default(),
            hotkeys: HotkeySettings::default(),
            inspector: InspectorSettings::default(),
            reconnect: ReconnectSettings::default(),
            updates: UpdateSettings::default(),
            log_level: "info".into(),
            active_account: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for a newer release at launch and once a day.
    pub check_automatically: bool,
    /// A version the user chose to skip: not offered again automatically.
    pub skipped_version: Option<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check_automatically: true, skipped_version: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NotificationSettings {
    pub tunnel_online: bool,
    pub tunnel_down: bool,
    pub tunnel_reconnected: bool,
    pub errors: bool,
    pub url_copied: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { tunnel_online: true, tunnel_down: true, tunnel_reconnected: true, errors: true, url_copied: false }
    }
}

/// Global shortcuts, written as `cmd+shift+KeyS`-style accelerators
/// (modifiers: `cmd`, `shift`, `alt`/`option`, `ctrl`; key: a `KeyX`,
/// `DigitN`, `F1`… code). Empty string = disabled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HotkeySettings {
    pub toggle_popover: String,
    pub copy_last_url: String,
    /// Per-favorite toggles: profile id → accelerator.
    pub profiles: Vec<ProfileHotkey>,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self { toggle_popover: "cmd+shift+KeyN".into(), copy_last_url: String::new(), profiles: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileHotkey {
    pub profile_id: Uuid,
    pub accelerator: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InspectorSettings {
    /// Requests kept per tunnel (ring buffer).
    pub max_requests: usize,
    /// Bytes of each body kept for display/replay.
    pub max_body_bytes: usize,
    /// Drop captured traffic when the tunnel stops.
    pub clear_on_stop: bool,
}

impl Default for InspectorSettings {
    fn default() -> Self {
        Self { max_requests: 500, max_body_bytes: 1024 * 1024, clear_on_stop: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReconnectSettings {
    /// Failed reconnect attempts before a session (and its tunnels) is
    /// declared failed.
    pub max_attempts: u32,
    /// Heartbeat interval in seconds (lower = faster dead-connection detection).
    pub heartbeat_interval_secs: u64,
    /// Heartbeat tolerance in seconds.
    pub heartbeat_tolerance_secs: u64,
}

/// ngrok rejects sessions with a shorter heartbeat interval (ERR_NGROK_111).
pub const MIN_HEARTBEAT_SECS: u64 = 10;

impl Default for ReconnectSettings {
    fn default() -> Self {
        Self { max_attempts: 10, heartbeat_interval_secs: MIN_HEARTBEAT_SECS, heartbeat_tolerance_secs: 15 }
    }
}

impl ReconnectSettings {
    /// Heartbeat (interval, tolerance) clamped to what ngrok accepts, so an
    /// old or hand-edited config cannot make every session fail.
    pub fn heartbeat(&self) -> (u64, u64) {
        let interval = self.heartbeat_interval_secs.max(MIN_HEARTBEAT_SECS);
        let tolerance = self.heartbeat_tolerance_secs.max(interval + 5);
        (interval, tolerance)
    }
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

/// A secret string. Never printed by `Debug`, never serialized.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str("Secret(<empty>)")
        } else {
            f.write_str("Secret(<redacted>)")
        }
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}
impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    /// Optional label colour as `#rrggbb`.
    #[serde(default)]
    pub color: Option<String>,
    /// Agent connect address override (`host:port`), e.g. for a region.
    #[serde(default)]
    pub server_addr: Option<String>,
    /// HTTP(S)/SOCKS5 proxy for the agent connection.
    #[serde(default)]
    pub proxy_url: Option<String>,
    /// PEM file with root CAs, or `trusted`/`host`.
    #[serde(default)]
    pub root_cas: Option<String>,
    #[serde(skip)]
    pub authtoken: Secret,
    #[serde(skip)]
    pub api_key: Secret,
}

impl Account {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            color: None,
            server_addr: None,
            proxy_url: None,
            root_cas: None,
            authtoken: Secret::default(),
            api_key: Secret::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tunnel profiles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Binding {
    #[default]
    Public,
    Internal,
    Kubernetes,
}

impl Binding {
    pub const ALL: [Binding; 3] = [Binding::Public, Binding::Internal, Binding::Kubernetes];
    pub fn as_str(self) -> &'static str {
        match self {
            Binding::Public => "public",
            Binding::Internal => "internal",
            Binding::Kubernetes => "kubernetes",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtoVersion {
    #[default]
    None,
    V1,
    V2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    Tcp,
    Tls,
}

impl Protocol {
    pub const ALL: [Protocol; 3] = [Protocol::Http, Protocol::Tcp, Protocol::Tls];
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Tcp => "tcp",
            Protocol::Tls => "tls",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Http => "HTTP",
            Protocol::Tcp => "TCP",
            Protocol::Tls => "TLS",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TunnelProfile {
    pub id: Uuid,
    pub name: String,
    pub account_id: Uuid,
    #[serde(default)]
    pub favorite: bool,
    /// Capture HTTP traffic for the inspector (HTTP tunnels only).
    #[serde(default = "default_true")]
    pub inspect: bool,
    /// Local service: a port (`3000`), `host:port`, or a URL
    /// (`http://`, `https://`, `tcp://`, `unix:///path`).
    pub upstream: String,
    /// Public URL / domain / reserved address. Empty = ngrok assigns one.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub metadata: String,
    /// Shown as "forwards to" in the ngrok dashboard. Empty = upstream.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub binding: Binding,
    #[serde(default)]
    pub pooling: bool,
    /// Traffic policy document, YAML or JSON. Empty = none.
    #[serde(default)]
    pub traffic_policy: String,
    #[serde(default)]
    pub proxy_proto: ProxyProtoVersion,
    #[serde(default)]
    pub allow_cidrs: Vec<String>,
    #[serde(default)]
    pub deny_cidrs: Vec<String>,
    /// Verify the upstream TLS certificate (https upstreams).
    #[serde(default = "default_true")]
    pub verify_upstream_tls: bool,
    pub options: ProtocolOptions,
}

fn default_true() -> bool {
    true
}

impl TunnelProfile {
    pub fn new(name: impl Into<String>, account_id: Uuid, protocol: Protocol, upstream: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            account_id,
            favorite: false,
            inspect: true,
            upstream: upstream.into(),
            url: String::new(),
            metadata: String::new(),
            description: String::new(),
            binding: Binding::Public,
            pooling: false,
            traffic_policy: String::new(),
            proxy_proto: ProxyProtoVersion::None,
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            verify_upstream_tls: true,
            options: ProtocolOptions::for_protocol(protocol),
        }
    }

    pub fn protocol(&self) -> Protocol {
        self.options.protocol()
    }

    /// Mutable references to every secret held by this profile, with a stable
    /// key suffix used for the secret store.
    pub fn secrets_mut(&mut self) -> Vec<(String, &mut Secret)> {
        let mut out = Vec::new();
        if let ProtocolOptions::Http(h) = &mut self.options {
            for (i, ba) in h.basic_auth.iter_mut().enumerate() {
                out.push((format!("basic_auth.{i}"), &mut ba.password));
            }
            if let Some(o) = &mut h.oauth {
                out.push(("oauth.client_secret".into(), &mut o.client_secret));
            }
            if let Some(o) = &mut h.oidc {
                out.push(("oidc.client_secret".into(), &mut o.client_secret));
            }
            if let Some(w) = &mut h.webhook_verification {
                out.push(("webhook.secret".into(), &mut w.secret));
            }
        }
        out
    }
}

#[allow(clippy::large_enum_variant)] // plain config data, rarely moved
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProtocolOptions {
    Http(HttpOptions),
    Tcp(TcpOptions),
    Tls(TlsOptions),
}

impl ProtocolOptions {
    pub fn for_protocol(p: Protocol) -> Self {
        match p {
            Protocol::Http => Self::Http(HttpOptions::default()),
            Protocol::Tcp => Self::Tcp(TcpOptions::default()),
            Protocol::Tls => Self::Tls(TlsOptions::default()),
        }
    }
    pub fn protocol(&self) -> Protocol {
        match self {
            Self::Http(_) => Protocol::Http,
            Self::Tcp(_) => Protocol::Tcp,
            Self::Tls(_) => Protocol::Tls,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct HttpOptions {
    /// `http` or `https` (default) for the public endpoint.
    pub scheme: Option<String>,
    pub basic_auth: Vec<BasicAuth>,
    pub oauth: Option<OAuth>,
    pub oidc: Option<Oidc>,
    pub webhook_verification: Option<WebhookVerification>,
    pub request_headers_add: Vec<HeaderPair>,
    pub request_headers_remove: Vec<String>,
    pub response_headers_add: Vec<HeaderPair>,
    pub response_headers_remove: Vec<String>,
    /// Rewrite the Host header to the upstream host.
    pub host_header_rewrite: bool,
    pub compression: bool,
    /// Error ratio (0..1) at which the circuit breaker trips. None = off.
    pub circuit_breaker: Option<f64>,
    pub websocket_tcp_conversion: bool,
    /// `http1` (default) or `http2` towards the upstream.
    pub app_protocol: Option<String>,
    /// Path to a PEM bundle of CAs for mutual TLS at the edge.
    pub mutual_tls_ca_path: Option<String>,
    pub allow_user_agents: Vec<String>,
    pub deny_user_agents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BasicAuth {
    pub username: String,
    #[serde(skip)]
    pub password: Secret,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct OAuth {
    /// `google`, `github`, `gitlab`, `microsoft`, `linkedin`, `twitch`, …
    pub provider: String,
    pub allow_emails: Vec<String>,
    pub allow_domains: Vec<String>,
    pub scopes: Vec<String>,
    pub client_id: String,
    #[serde(skip)]
    pub client_secret: Secret,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Oidc {
    pub issuer_url: String,
    pub client_id: String,
    #[serde(skip)]
    pub client_secret: Secret,
    pub allow_emails: Vec<String>,
    pub allow_domains: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WebhookVerification {
    /// `github`, `stripe`, `slack`, `twilio`, `shopify`, …
    pub provider: String,
    #[serde(skip)]
    pub secret: Secret,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

impl HeaderPair {
    /// Parse `Name: value` (also accepts `Name:value` and `Name=value`).
    pub fn parse(s: &str) -> Option<Self> {
        let (name, value) = s.split_once(':').or_else(|| s.split_once('='))?;
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        Some(Self { name: name.into(), value: value.trim().into() })
    }
}

impl fmt::Display for HeaderPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct TcpOptions {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct TlsOptions {
    /// Terminate TLS at the edge with this certificate (PEM path). If both
    /// `cert_path` and `key_path` are empty the TLS stream is passed through.
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub mutual_tls_ca_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        let s = Secret::new("2abc_supersecret");
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        let a = Account { authtoken: s, ..Account::new("x") };
        assert!(!format!("{a:?}").contains("supersecret"));
    }

    #[test]
    fn secrets_are_not_serialized() {
        let mut cfg = AppConfig::default();
        let mut acc = Account::new("me");
        acc.authtoken = "tok_123".into();
        acc.api_key = "key_456".into();
        let mut p = TunnelProfile::new("web", acc.id, Protocol::Http, "3000");
        if let ProtocolOptions::Http(h) = &mut p.options {
            h.basic_auth.push(BasicAuth { username: "u".into(), password: "pw_789".into() });
        }
        cfg.accounts.push(acc);
        cfg.profiles.push(p);
        let s = toml::to_string(&cfg).unwrap();
        assert!(!s.contains("tok_123"));
        assert!(!s.contains("key_456"));
        assert!(!s.contains("pw_789"));
        let back: AppConfig = toml::from_str(&s).unwrap();
        assert_eq!(back.profiles[0].name, "web");
    }

    #[test]
    fn heartbeat_is_clamped_to_ngrok_minimum() {
        assert_eq!(ReconnectSettings::default().heartbeat(), (10, 15));
        let old = ReconnectSettings { max_attempts: 3, heartbeat_interval_secs: 5, heartbeat_tolerance_secs: 10 };
        assert_eq!(old.heartbeat(), (10, 15));
        let custom = ReconnectSettings { max_attempts: 3, heartbeat_interval_secs: 30, heartbeat_tolerance_secs: 60 };
        assert_eq!(custom.heartbeat(), (30, 60));
    }

    #[test]
    fn header_pair_parse() {
        assert_eq!(HeaderPair::parse("X-A: b c").unwrap(), HeaderPair { name: "X-A".into(), value: "b c".into() });
        assert_eq!(HeaderPair::parse("k=v").unwrap().value, "v");
        assert!(HeaderPair::parse(": nope").is_none());
    }

    #[test]
    fn remove_account_cascades() {
        let mut cfg = AppConfig::default();
        let a = Account::new("a");
        let b = Account::new("b");
        cfg.profiles.push(TunnelProfile::new("pa", a.id, Protocol::Http, "1"));
        cfg.profiles.push(TunnelProfile::new("pb", b.id, Protocol::Tcp, "2"));
        cfg.settings.active_account = Some(a.id);
        let aid = a.id;
        cfg.accounts.extend([a, b]);
        let removed = cfg.remove_account(aid);
        assert_eq!(removed.len(), 1);
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.settings.active_account, None);
        assert_eq!(cfg.active_account().unwrap().name, "b");
    }
}
