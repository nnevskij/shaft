//! Upstream address normalisation and profile validation.

use url::Url;

use crate::model::{Protocol, ProtocolOptions, TunnelProfile};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum UpstreamError {
    #[error("upstream is empty")]
    Empty,
    #[error("invalid port: {0}")]
    InvalidPort(String),
    #[error("invalid upstream address: {0}")]
    Invalid(String),
    #[error("scheme `{0}` is not supported for {1} tunnels")]
    UnsupportedScheme(String, &'static str),
}

/// Turn a user-supplied upstream (`3000`, `host:3000`, `https://x:8443`,
/// `unix:///tmp/s.sock`) into a URL usable for forwarding.
pub fn normalize_upstream(input: &str, protocol: Protocol) -> Result<Url, UpstreamError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(UpstreamError::Empty);
    }
    let default_scheme = match protocol {
        Protocol::Http => "http",
        Protocol::Tcp | Protocol::Tls => "tcp",
    };

    // Bare port.
    if s.chars().all(|c| c.is_ascii_digit()) {
        let port = parse_port(s)?;
        return Url::parse(&format!("{default_scheme}://localhost:{port}")).map_err(|e| UpstreamError::Invalid(e.to_string()));
    }

    let url = if s.contains("://") || s.starts_with("unix:") {
        Url::parse(s).map_err(|e| UpstreamError::Invalid(e.to_string()))?
    } else {
        // host:port (or [v6]:port) without scheme.
        let (_, port) = s.rsplit_once(':').ok_or_else(|| UpstreamError::Invalid(format!("`{s}` needs a port")))?;
        parse_port(port)?;
        Url::parse(&format!("{default_scheme}://{s}")).map_err(|e| UpstreamError::Invalid(e.to_string()))?
    };

    let scheme = url.scheme().to_string();
    let allowed: &[&str] = match protocol {
        Protocol::Http => &["http", "https", "unix"],
        Protocol::Tcp | Protocol::Tls => &["tcp", "tls", "unix"],
    };
    if !allowed.contains(&scheme.as_str()) {
        return Err(UpstreamError::UnsupportedScheme(scheme, protocol.label()));
    }
    if scheme != "unix" && url.host_str().is_none() {
        return Err(UpstreamError::Invalid(format!("`{s}` has no host")));
    }
    if scheme != "unix" && url.port_or_known_default().is_none() {
        return Err(UpstreamError::Invalid(format!("`{s}` has no port")));
    }
    Ok(url)
}

fn parse_port(s: &str) -> Result<u16, UpstreamError> {
    match s.parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => Ok(p as u16),
        _ => Err(UpstreamError::InvalidPort(s.into())),
    }
}

/// Host and port to probe for reachability (None for unix sockets).
pub fn probe_addr(url: &Url) -> Option<(String, u16)> {
    if url.scheme() == "unix" {
        return None;
    }
    let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').to_string();
    Some((
        host,
        url.port_or_known_default().unwrap_or(match url.scheme() {
            "https" | "tls" => 443,
            _ => 80,
        }),
    ))
}

/// Convert a YAML or JSON traffic policy into compact JSON. Empty input → None.
pub fn traffic_policy_to_json(input: &str) -> Result<Option<String>, String> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value = if s.starts_with('{') {
        serde_json::from_str(s).map_err(|e| format!("invalid JSON policy: {e}"))?
    } else {
        let yaml: serde_yaml::Value = serde_yaml::from_str(s).map_err(|e| format!("invalid YAML policy: {e}"))?;
        serde_json::to_value(yaml).map_err(|e| format!("policy cannot be represented as JSON: {e}"))?
    };
    let obj = value.as_object().ok_or("traffic policy must be a mapping")?;
    const KNOWN: [&str; 3] = ["on_http_request", "on_http_response", "on_tcp_connect"];
    if let Some(k) = obj.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(format!("unknown traffic policy phase `{k}` (expected one of {})", KNOWN.join(", ")));
    }
    for (phase, rules) in obj {
        let rules = rules.as_array().ok_or_else(|| format!("`{phase}` must be a list of rules"))?;
        for (i, rule) in rules.iter().enumerate() {
            let actions = rule.get("actions").and_then(|a| a.as_array());
            if actions.map(|a| a.is_empty()).unwrap_or(true) {
                return Err(format!("`{phase}[{i}]` needs a non-empty `actions` list"));
            }
            for (j, a) in actions.unwrap().iter().enumerate() {
                if a.get("type").and_then(|t| t.as_str()).is_none() {
                    return Err(format!("`{phase}[{i}].actions[{j}]` needs a `type`"));
                }
            }
        }
    }
    Ok(Some(value.to_string()))
}

/// Client-side validation run before saving or starting a profile.
pub fn validate_profile(p: &TunnelProfile) -> Vec<String> {
    let mut errs = Vec::new();
    if p.name.trim().is_empty() {
        errs.push("name is required".into());
    }
    if let Err(e) = normalize_upstream(&p.upstream, p.protocol()) {
        errs.push(format!("upstream: {e}"));
    }
    if let Err(e) = traffic_policy_to_json(&p.traffic_policy) {
        errs.push(format!("traffic policy: {e}"));
    }
    for c in p.allow_cidrs.iter().chain(&p.deny_cidrs) {
        if !is_cidr(c) {
            errs.push(format!("`{c}` is not a valid CIDR"));
        }
    }
    match &p.options {
        ProtocolOptions::Http(h) => {
            if let Some(s) = &h.scheme {
                if !matches!(s.as_str(), "http" | "https") {
                    errs.push(format!("scheme `{s}` must be http or https"));
                }
            }
            if let Some(cb) = h.circuit_breaker {
                if !(0.0..=1.0).contains(&cb) {
                    errs.push("circuit breaker ratio must be between 0 and 1".into());
                }
            }
            for b in &h.basic_auth {
                if b.username.is_empty() || b.username.contains(':') {
                    errs.push("basic auth username must be non-empty and not contain ':'".into());
                }
                if b.password.expose().len() < 8 || b.password.expose().len() > 128 {
                    errs.push(format!("basic auth password for `{}` must be 8-128 characters", b.username));
                }
            }
            if let Some(o) = &h.oauth {
                if o.provider.trim().is_empty() {
                    errs.push("OAuth provider is required".into());
                }
            }
            if let Some(o) = &h.oidc {
                if o.issuer_url.is_empty() || o.client_id.is_empty() || o.client_secret.is_empty() {
                    errs.push("OIDC needs issuer URL, client id and client secret".into());
                }
            }
            if let Some(w) = &h.webhook_verification {
                if w.provider.is_empty() || w.secret.is_empty() {
                    errs.push("webhook verification needs provider and secret".into());
                }
            }
            if let Some(ap) = &h.app_protocol {
                if !matches!(ap.as_str(), "http1" | "http2") {
                    errs.push("app protocol must be http1 or http2".into());
                }
            }
        }
        ProtocolOptions::Tcp(_) => {
            if !p.url.is_empty() && !p.url.starts_with("tcp://") && !p.url.contains(':') {
                errs.push("TCP url must be a reserved address like 1.tcp.ngrok.io:12345".into());
            }
        }
        ProtocolOptions::Tls(t) => {
            if t.cert_path.is_some() != t.key_path.is_some() {
                errs.push("TLS termination needs both certificate and key".into());
            }
        }
    }
    errs
}

fn is_cidr(s: &str) -> bool {
    let (ip, bits) = match s.split_once('/') {
        Some((ip, bits)) => (ip, bits),
        None => return false,
    };
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(_)) => bits.parse::<u8>().map(|b| b <= 32).unwrap_or(false),
        Ok(std::net::IpAddr::V6(_)) => bits.parse::<u8>().map(|b| b <= 128).unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    #[test]
    fn normalizes() {
        let h = Protocol::Http;
        assert_eq!(normalize_upstream("3000", h).unwrap().as_str(), "http://localhost:3000/");
        assert_eq!(normalize_upstream("192.168.1.2:80", h).unwrap().as_str(), "http://192.168.1.2/");
        assert_eq!(normalize_upstream("https://local.dev:8443", h).unwrap().as_str(), "https://local.dev:8443/");
        assert_eq!(normalize_upstream("22", Protocol::Tcp).unwrap().as_str(), "tcp://localhost:22");
        assert_eq!(normalize_upstream("unix:///tmp/a.sock", h).unwrap().scheme(), "unix");
        assert!(matches!(normalize_upstream("0", h), Err(UpstreamError::InvalidPort(_))));
        assert!(matches!(normalize_upstream("70000", h), Err(UpstreamError::InvalidPort(_))));
        assert!(matches!(normalize_upstream("tcp://x:1", h), Err(UpstreamError::UnsupportedScheme(..))));
        assert!(normalize_upstream("localhost", h).is_err());
        assert_eq!(normalize_upstream("", h), Err(UpstreamError::Empty));
    }

    #[test]
    fn probe() {
        let u = normalize_upstream("https://x.test", Protocol::Http).unwrap();
        assert_eq!(probe_addr(&u), Some(("x.test".into(), 443)));
        let u = normalize_upstream("[::1]:8080", Protocol::Http).unwrap();
        assert_eq!(probe_addr(&u), Some(("::1".into(), 8080)));
    }

    #[test]
    fn policy_yaml_to_json() {
        let y = "on_http_request:\n  - actions:\n      - type: deny\n";
        let j = traffic_policy_to_json(y).unwrap().unwrap();
        assert!(j.contains("\"deny\""));
        assert_eq!(traffic_policy_to_json("  ").unwrap(), None);
        assert!(traffic_policy_to_json("on_foo: []").is_err());
        assert!(traffic_policy_to_json("on_http_request:\n  - expressions: []\n").is_err());
        assert!(traffic_policy_to_json("[1,2]").is_err());
    }

    #[test]
    fn validates() {
        let mut p = TunnelProfile::new("", uuid::Uuid::new_v4(), Protocol::Http, "abc");
        p.allow_cidrs.push("10.0.0.0/33".into());
        let errs = validate_profile(&p);
        assert_eq!(errs.len(), 3, "{errs:?}");
        p.name = "ok".into();
        p.upstream = "8080".into();
        p.allow_cidrs = vec!["10.0.0.0/8".into(), "::/0".into()];
        assert!(validate_profile(&p).is_empty());
    }
}
