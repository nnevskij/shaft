//! Translate a [`TunnelProfile`] into ngrok SDK endpoint builders.

use bytes::Bytes;
use ngrok::config::{HttpTunnelBuilder, OauthOptions, OidcOptions, ProxyProto, Scheme, TcpTunnelBuilder, TlsTunnelBuilder};
use shaft_config::{Binding, HttpOptions, ProxyProtoVersion, TlsOptions, TunnelProfile};
use url::Url;

use crate::model::CoreError;

fn read_file(path: &str, what: &str) -> Result<Bytes, CoreError> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_else(|_| path.to_string())
    } else {
        path.to_string()
    };
    std::fs::read(&expanded).map(Bytes::from).map_err(|e| CoreError::new(format!("cannot read {what} `{path}`: {e}")))
}

fn proxy_proto(v: ProxyProtoVersion) -> ProxyProto {
    match v {
        ProxyProtoVersion::None => ProxyProto::None,
        ProxyProtoVersion::V1 => ProxyProto::V1,
        ProxyProtoVersion::V2 => ProxyProto::V2,
    }
}

/// Options every endpoint type supports, applied through a macro because the
/// SDK builders share method names but not a trait.
macro_rules! apply_common {
    ($b:expr, $p:expr, $policy:expr, $upstream:expr) => {{
        let p: &TunnelProfile = $p;
        if !p.metadata.is_empty() {
            $b.metadata(p.metadata.clone());
        }
        let forwards_to = if p.description.is_empty() { $upstream.to_string() } else { p.description.clone() };
        $b.forwards_to(forwards_to);
        if p.binding != Binding::Public {
            $b.binding(p.binding.as_str());
        }
        if p.pooling {
            $b.pooling_enabled(true);
        }
        if let Some(policy) = $policy {
            $b.traffic_policy(policy);
        }
        for c in &p.allow_cidrs {
            $b.allow_cidr(c.clone());
        }
        for c in &p.deny_cidrs {
            $b.deny_cidr(c.clone());
        }
        if p.proxy_proto != ProxyProtoVersion::None {
            $b.proxy_proto(proxy_proto(p.proxy_proto));
        }
        $b.verify_upstream_tls(p.verify_upstream_tls);
    }};
}

/// Split a profile URL like `https://foo.ngrok.app` into (scheme, host).
fn split_http_url(url: &str) -> (Option<&str>, &str) {
    match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest.trim_end_matches('/')),
        None => (None, url.trim_end_matches('/')),
    }
}

pub fn apply_http(
    b: &mut HttpTunnelBuilder,
    p: &TunnelProfile,
    h: &HttpOptions,
    policy: Option<String>,
    upstream: &Url,
) -> Result<(), CoreError> {
    apply_common!(b, p, policy, upstream);
    if !p.url.is_empty() {
        let (scheme, host) = split_http_url(&p.url);
        b.domain(host.to_string());
        if let Some(s) = scheme {
            b.scheme(if s.eq_ignore_ascii_case("http") { Scheme::HTTP } else { Scheme::HTTPS });
        }
    }
    if let Some(s) = &h.scheme {
        b.scheme(if s == "http" { Scheme::HTTP } else { Scheme::HTTPS });
    }
    for ba in &h.basic_auth {
        b.basic_auth(ba.username.clone(), ba.password.expose().to_string());
    }
    if let Some(o) = &h.oauth {
        let mut opts = OauthOptions::new(o.provider.clone());
        if !o.client_id.is_empty() {
            opts.client_id(o.client_id.clone()).client_secret(o.client_secret.expose().to_string());
        }
        for e in &o.allow_emails {
            opts.allow_email(e.clone());
        }
        for d in &o.allow_domains {
            opts.allow_domain(d.clone());
        }
        for s in &o.scopes {
            opts.scope(s.clone());
        }
        b.oauth(opts);
    }
    if let Some(o) = &h.oidc {
        let mut opts = OidcOptions::new(o.issuer_url.clone(), o.client_id.clone(), o.client_secret.expose().to_string());
        for e in &o.allow_emails {
            opts.allow_email(e.clone());
        }
        for d in &o.allow_domains {
            opts.allow_domain(d.clone());
        }
        for s in &o.scopes {
            opts.scope(s.clone());
        }
        b.oidc(opts);
    }
    if let Some(w) = &h.webhook_verification {
        b.webhook_verification(w.provider.clone(), w.secret.expose().to_string());
    }
    for hp in &h.request_headers_add {
        b.request_header(hp.name.clone(), hp.value.clone());
    }
    for n in &h.request_headers_remove {
        b.remove_request_header(n.clone());
    }
    for hp in &h.response_headers_add {
        b.response_header(hp.name.clone(), hp.value.clone());
    }
    for n in &h.response_headers_remove {
        b.remove_response_header(n.clone());
    }
    if h.host_header_rewrite {
        // The SDK applies this only in listen_and_forward; the inspector
        // proxy rewrites Host itself for inspected tunnels.
        b.host_header_rewrite(true);
        if let Some(host) = upstream.host_str() {
            let host = match upstream.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
            b.request_header("host", host);
        }
    }
    if h.compression {
        b.compression();
    }
    if let Some(cb) = h.circuit_breaker {
        b.circuit_breaker(cb);
    }
    if h.websocket_tcp_conversion {
        b.websocket_tcp_conversion();
    }
    if let Some(ap) = &h.app_protocol {
        b.app_protocol(ap.clone());
    }
    if let Some(path) = &h.mutual_tls_ca_path {
        b.mutual_tlsca(read_file(path, "mutual TLS CA bundle")?);
    }
    for re in &h.allow_user_agents {
        b.allow_user_agent(re.clone());
    }
    for re in &h.deny_user_agents {
        b.deny_user_agent(re.clone());
    }
    Ok(())
}

pub fn apply_tcp(b: &mut TcpTunnelBuilder, p: &TunnelProfile, policy: Option<String>, upstream: &Url) -> Result<(), CoreError> {
    apply_common!(b, p, policy, upstream);
    if !p.url.is_empty() {
        b.remote_addr(p.url.trim_start_matches("tcp://").to_string());
    }
    Ok(())
}

pub fn apply_tls(
    b: &mut TlsTunnelBuilder,
    p: &TunnelProfile,
    t: &TlsOptions,
    policy: Option<String>,
    upstream: &Url,
) -> Result<(), CoreError> {
    apply_common!(b, p, policy, upstream);
    if !p.url.is_empty() {
        b.domain(p.url.trim_start_matches("tls://").trim_end_matches('/').to_string());
    }
    if let (Some(cert), Some(key)) = (&t.cert_path, &t.key_path) {
        b.termination(read_file(cert, "certificate")?, read_file(key, "private key")?);
    }
    if let Some(path) = &t.mutual_tls_ca_path {
        b.mutual_tlsca(read_file(path, "mutual TLS CA bundle")?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        assert_eq!(split_http_url("https://a.ngrok.app/"), (Some("https"), "a.ngrok.app"));
        assert_eq!(split_http_url("a.example.com"), (None, "a.example.com"));
    }

    #[test]
    fn reads_files_with_tilde() {
        assert!(read_file("~/definitely/missing.pem", "x").unwrap_err().message.contains("missing.pem"));
    }
}
