//! Connectivity diagnostics (the GUI counterpart of `ngrok diagnose`).

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ngrok::Session;
use rustls::pki_types::ServerName;
use shaft_config::Account;
use tokio::net::TcpStream;

pub const DEFAULT_CONNECT_ADDR: &str = "connect.ngrok-agent.com:443";
/// ngrok's agent CA, copied from the ngrok-rust SDK (`assets/ngrok.ca.crt`).
const NGROK_CA: &[u8] = include_bytes!("../assets/ngrok.ca.crt");
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warning,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub duration: Option<Duration>,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>, d: Duration) -> Self {
        Self { name: name.into(), status: CheckStatus::Ok, detail: detail.into(), duration: Some(d) }
    }
    fn failed(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: CheckStatus::Failed, detail: detail.into(), duration: None }
    }
    fn skipped(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: CheckStatus::Skipped, detail: detail.into(), duration: None }
    }
    fn symbol(&self) -> &'static str {
        match self.status {
            CheckStatus::Ok => "✓",
            CheckStatus::Warning => "!",
            CheckStatus::Failed => "✗",
            CheckStatus::Skipped => "–",
        }
    }
}

fn split_addr(addr: &str) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(443)),
        None => (addr.to_string(), 443),
    }
}

async fn dns(host: &str, port: u16) -> Check {
    let start = Instant::now();
    match tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host, port))).await {
        Ok(Ok(addrs)) => {
            let addrs: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
            Check::ok("DNS", format!("{host} → {}", addrs.join(", ")), start.elapsed())
        }
        Ok(Err(e)) => Check::failed("DNS", format!("{host}: {e}")),
        Err(_) => Check::failed("DNS", format!("{host}: timed out")),
    }
}

async fn tcp_tls(host: &str, port: u16) -> (Check, Check) {
    let start = Instant::now();
    let stream = match tokio::time::timeout(TIMEOUT, TcpStream::connect((host, port))).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return (Check::failed("TCP", format!("{host}:{port}: {e}")), Check::skipped("TLS", "no TCP connection")),
        Err(_) => return (Check::failed("TCP", format!("{host}:{port}: timed out")), Check::skipped("TLS", "no TCP connection")),
    };
    let tcp = Check::ok("TCP", format!("connected to {}", stream.peer_addr().map(|a| a.to_string()).unwrap_or_default()), start.elapsed());

    // The agent edge is signed by ngrok's own CA (bundled with the SDK);
    // system roots are added for custom server addresses.
    let mut roots = rustls::RootCertStore::empty();
    {
        use rustls::pki_types::{pem::PemObject, CertificateDer};
        for c in CertificateDer::pem_slice_iter(NGROK_CA).flatten() {
            let _ = roots.add(c);
        }
    }
    for c in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(c);
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let cfg = match rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions() {
        Ok(b) => b.with_root_certificates(roots).with_no_client_auth(),
        Err(e) => return (tcp, Check::failed("TLS", e.to_string())),
    };
    let name = match ServerName::try_from(host.to_string()) {
        Ok(n) => n,
        Err(e) => return (tcp, Check::failed("TLS", e.to_string())),
    };
    let start = Instant::now();
    let tls = match tokio::time::timeout(TIMEOUT, tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, stream)).await {
        Ok(Ok(s)) => {
            let (_, conn) = s.get_ref();
            let v = conn.protocol_version().map(|v| format!("{v:?}")).unwrap_or_default();
            Check::ok("TLS", format!("handshake ok ({v})"), start.elapsed())
        }
        Ok(Err(e)) => Check::failed("TLS", format!("{e} — a proxy or firewall may be intercepting TLS")),
        Err(_) => Check::failed("TLS", "handshake timed out"),
    };
    (tcp, tls)
}

pub async fn check_authtoken(account: &Account) -> Check {
    if account.authtoken.is_empty() {
        return Check::skipped("Authtoken", "not configured");
    }
    let start = Instant::now();
    let mut b = Session::builder();
    b.authtoken(account.authtoken.expose().to_string()).client_info("shaft-diagnose", env!("CARGO_PKG_VERSION"), None::<String>);
    if let Some(addr) = account.server_addr.as_deref().filter(|s| !s.is_empty()) {
        if let Err(e) = b.server_addr(addr) {
            return Check::failed("Authtoken", e.to_string());
        }
    }
    // Do not retry forever while diagnosing.
    b.connector(|host: String, port: u16, tls, err: Option<ngrok::tunnel::AcceptError>| async move {
        if err.is_some() {
            return Err(ngrok::session::ConnectError::Canceled);
        }
        ngrok::session::default_connect(host, port, tls, None).await
    });
    match tokio::time::timeout(TIMEOUT, b.connect()).await {
        Ok(Ok(mut s)) => {
            let id = s.id();
            let _ = s.close().await;
            Check::ok("Authtoken", format!("session {id} opened"), start.elapsed())
        }
        Ok(Err(e)) => {
            let code = ngrok::Error::error_code(&e).map(|c| format!(" ({c})")).unwrap_or_default();
            Check::failed("Authtoken", format!("{}{code}", ngrok::Error::msg(&e)))
        }
        Err(_) => Check::failed("Authtoken", "timed out opening a session"),
    }
}

pub async fn check_api_key(account: &Account) -> Check {
    if account.api_key.is_empty() {
        return Check::skipped("API key", "not configured");
    }
    let start = Instant::now();
    match shaft_api::ApiClient::new(account.api_key.expose()).validate().await {
        Ok(()) => Check::ok("API key", "api.ngrok.com accepted the key", start.elapsed()),
        Err(e) => Check::failed("API key", e.to_string()),
    }
}

/// Run every check for an account (connectivity checks use its server address).
pub async fn run(account: Option<&Account>) -> Vec<Check> {
    let addr = account.and_then(|a| a.server_addr.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_CONNECT_ADDR.to_string());
    let (host, port) = split_addr(&addr);
    let mut checks = vec![dns(&host, port).await];
    if checks[0].status == CheckStatus::Ok {
        let (tcp, tls) = tcp_tls(&host, port).await;
        checks.push(tcp);
        checks.push(tls);
    } else {
        checks.push(Check::skipped("TCP", "DNS failed"));
        checks.push(Check::skipped("TLS", "DNS failed"));
    }
    match account {
        Some(a) => {
            let (t, k) = tokio::join!(check_authtoken(a), check_api_key(a));
            checks.push(t);
            checks.push(k);
        }
        None => {
            checks.push(Check::skipped("Authtoken", "no account"));
            checks.push(Check::skipped("API key", "no account"));
        }
    }
    checks
}

/// Plain-text report, safe to share (no secrets).
pub fn report(checks: &[Check]) -> String {
    let mut s = format!("Shaft {} diagnostics\nmacOS {}\nngrok-rust SDK 0.19\n\n", env!("CARGO_PKG_VERSION"), os_version());
    for c in checks {
        let d = c.duration.map(|d| format!(" [{} ms]", d.as_millis())).unwrap_or_default();
        s.push_str(&format!("{} {:<10} {}{d}\n", c.symbol(), c.name, c.detail));
    }
    s
}

fn os_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_split() {
        assert_eq!(split_addr("connect.ngrok-agent.com:443"), ("connect.ngrok-agent.com".into(), 443));
        assert_eq!(split_addr("host"), ("host".into(), 443));
    }

    #[test]
    fn report_has_no_secrets() {
        let checks = vec![Check::ok("DNS", "x → 1.2.3.4", Duration::from_millis(3)), Check::skipped("API key", "not configured")];
        let r = report(&checks);
        assert!(r.contains("✓ DNS"));
        assert!(r.contains("– API key"));
    }
}
