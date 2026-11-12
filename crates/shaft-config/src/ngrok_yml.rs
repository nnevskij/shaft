//! Import from / export to the ngrok agent configuration file (`ngrok.yml`).
//!
//! Import understands the v3 format (`agent:` + `endpoints:`) and the legacy
//! v2 format (top-level `authtoken` + `tunnels:`); `tunnels:` is also accepted
//! inside a v3 file. Export always writes v3, translating options that have no
//! v3 field (basic auth, OAuth, IP restrictions, headers…) into equivalent
//! Traffic Policy rules.

use serde_json::{json, Map, Value as Json};
use serde_yaml::{Mapping, Value as Yaml};
use uuid::Uuid;

use crate::model::*;

#[derive(Debug, Default)]
pub struct ImportResult {
    pub authtoken: Option<String>,
    pub api_key: Option<String>,
    pub server_addr: Option<String>,
    pub proxy_url: Option<String>,
    pub root_cas: Option<String>,
    pub profiles: Vec<TunnelProfile>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("invalid YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("ngrok.yml must be a mapping at the top level")]
    NotAMapping,
    #[error("unsupported ngrok.yml version `{0}`")]
    Version(String),
}

fn get<'a>(m: &'a Mapping, k: &str) -> Option<&'a Yaml> {
    m.get(Yaml::String(k.into()))
}

fn get_str(m: &Mapping, k: &str) -> Option<String> {
    match get(m, k)? {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Number(n) => Some(n.to_string()),
        Yaml::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn get_bool(m: &Mapping, k: &str) -> Option<bool> {
    match get(m, k)? {
        Yaml::Bool(b) => Some(*b),
        Yaml::String(s) => match s.as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn get_list(m: &Mapping, k: &str) -> Vec<String> {
    match get(m, k) {
        Some(Yaml::Sequence(s)) => s
            .iter()
            .filter_map(|v| match v {
                Yaml::String(s) => Some(s.clone()),
                Yaml::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Some(Yaml::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn get_map<'a>(m: &'a Mapping, k: &str) -> Option<&'a Mapping> {
    get(m, k)?.as_mapping()
}

/// Parse an ngrok.yml document. Profiles are bound to `account_id`.
pub fn import(text: &str, account_id: Uuid) -> Result<ImportResult, ImportError> {
    let doc: Yaml = serde_yaml::from_str(text)?;
    let root = doc.as_mapping().ok_or(ImportError::NotAMapping)?;
    let version = get_str(root, "version").unwrap_or_else(|| "2".into());
    let mut out = ImportResult::default();

    match version.trim_matches('"') {
        "1" | "2" => {
            out.authtoken = get_str(root, "authtoken");
            out.api_key = get_str(root, "api_key");
            out.server_addr = get_str(root, "server_addr");
            out.proxy_url = get_str(root, "proxy_url");
            out.root_cas = get_str(root, "root_cas");
            if let Some(region) = get_str(root, "region") {
                out.warnings.push(format!(
                    "region `{region}` ignored: ngrok now routes agents automatically (set a server address on the account if needed)"
                ));
            }
        }
        "3" => {
            if let Some(agent) = get_map(root, "agent") {
                out.authtoken = get_str(agent, "authtoken");
                out.api_key = get_str(agent, "api_key");
                out.server_addr = get_str(agent, "connect_url").or_else(|| get_str(agent, "server_addr"));
                out.proxy_url = get_str(agent, "proxy_url");
                out.root_cas = get_str(agent, "root_cas");
            }
        }
        other => return Err(ImportError::Version(other.into())),
    }

    if let Some(Yaml::Sequence(eps)) = get(root, "endpoints") {
        for (i, ep) in eps.iter().enumerate() {
            match ep.as_mapping() {
                Some(m) => out.profiles.push(import_endpoint(m, account_id, &mut out.warnings)),
                None => out.warnings.push(format!("endpoints[{i}] is not a mapping, skipped")),
            }
        }
    }
    if let Some(tunnels) = get_map(root, "tunnels") {
        for (name, def) in tunnels {
            let name = name.as_str().unwrap_or("tunnel").to_string();
            match def.as_mapping() {
                Some(m) => {
                    if let Some(p) = import_v2_tunnel(&name, m, account_id, &mut out.warnings) {
                        out.profiles.push(p)
                    }
                }
                None => out.warnings.push(format!("tunnel `{name}` is not a mapping, skipped")),
            }
        }
    }
    Ok(out)
}

fn yaml_policy_to_string(v: &Yaml) -> String {
    match v {
        Yaml::String(s) => s.clone(),
        other => serde_yaml::to_string(other).unwrap_or_default(),
    }
}

fn import_endpoint(m: &Mapping, account_id: Uuid, warnings: &mut Vec<String>) -> TunnelProfile {
    let name = get_str(m, "name").unwrap_or_else(|| "endpoint".into());
    let url = get_str(m, "url").unwrap_or_default();
    let protocol = if url.starts_with("tcp://") {
        Protocol::Tcp
    } else if url.starts_with("tls://") {
        Protocol::Tls
    } else {
        Protocol::Http
    };
    let upstream_map = get_map(m, "upstream");
    let upstream = upstream_map.and_then(|u| get_str(u, "url")).or_else(|| get_str(m, "upstream")).unwrap_or_default();
    let mut p = TunnelProfile::new(name.clone(), account_id, protocol, upstream);
    p.url = match protocol {
        Protocol::Http => url,
        Protocol::Tcp => url.trim_start_matches("tcp://").to_string(),
        Protocol::Tls => url.trim_start_matches("tls://").to_string(),
    };
    p.metadata = get_str(m, "metadata").unwrap_or_default();
    p.description = get_str(m, "description").unwrap_or_default();
    p.pooling = get_bool(m, "pooling_enabled").unwrap_or(false);
    if let Some(b) = get_list(m, "bindings").first() {
        match Binding::parse(b) {
            Some(b) => p.binding = b,
            None => warnings.push(format!("endpoint `{name}`: unknown binding `{b}`")),
        }
    }
    if let Some(tp) = get(m, "traffic_policy") {
        p.traffic_policy = yaml_policy_to_string(tp);
    }
    if let Some(u) = upstream_map {
        if let Some(pp) = get_str(u, "proxy_protocol") {
            p.proxy_proto = match pp.as_str() {
                "1" => ProxyProtoVersion::V1,
                "2" => ProxyProtoVersion::V2,
                _ => ProxyProtoVersion::None,
            };
        }
        if let (Some(proto), ProtocolOptions::Http(h)) = (get_str(u, "protocol"), &mut p.options) {
            h.app_protocol = Some(proto);
        }
    }
    p
}

fn import_v2_tunnel(name: &str, m: &Mapping, account_id: Uuid, warnings: &mut Vec<String>) -> Option<TunnelProfile> {
    let proto_s = get_str(m, "proto").unwrap_or_else(|| "http".into());
    let protocol = match Protocol::parse(&proto_s) {
        Some(p) => p,
        None => {
            warnings.push(format!("tunnel `{name}`: protocol `{proto_s}` is not supported, skipped"));
            return None;
        }
    };
    let addr = get_str(m, "addr").unwrap_or_default();
    let mut p = TunnelProfile::new(name, account_id, protocol, addr);
    p.metadata = get_str(m, "metadata").unwrap_or_default();
    p.inspect = get_bool(m, "inspect").unwrap_or(true);
    p.pooling = get_bool(m, "pooling_enabled").unwrap_or(false);
    if let Some(b) = get_str(m, "binding") {
        p.binding = Binding::parse(&b).unwrap_or_default();
    }
    if let Some(tp) = get(m, "traffic_policy").or_else(|| get(m, "policy")) {
        p.traffic_policy = yaml_policy_to_string(tp);
    }
    p.proxy_proto = match get_str(m, "proxy_proto").as_deref() {
        Some("1") => ProxyProtoVersion::V1,
        Some("2") => ProxyProtoVersion::V2,
        _ => ProxyProtoVersion::None,
    };
    if let Some(ipr) = get_map(m, "ip_restriction") {
        p.allow_cidrs = get_list(ipr, "allow_cidrs");
        p.deny_cidrs = get_list(ipr, "deny_cidrs");
    }
    p.url = get_str(m, "url")
        .or_else(|| get_str(m, "domain"))
        .or_else(|| get_str(m, "hostname"))
        .or_else(|| get_str(m, "remote_addr"))
        .unwrap_or_default();
    if let Some(sub) = get_str(m, "subdomain") {
        if p.url.is_empty() {
            p.url = format!("{sub}.ngrok.io");
            warnings.push(format!("tunnel `{name}`: subdomain converted to `{}` — verify the domain", p.url));
        }
    }

    match &mut p.options {
        ProtocolOptions::Http(h) => {
            for cred in get_list(m, "basic_auth").into_iter().chain(get_str(m, "auth")) {
                if let Some((u, pw)) = cred.split_once(':') {
                    h.basic_auth.push(BasicAuth { username: u.into(), password: pw.into() });
                }
            }
            if let Some(o) = get_map(m, "oauth") {
                h.oauth = Some(OAuth {
                    provider: get_str(o, "provider").unwrap_or_default(),
                    allow_emails: get_list(o, "allow_emails"),
                    allow_domains: get_list(o, "allow_domains"),
                    scopes: get_list(o, "scopes"),
                    client_id: get_str(o, "client_id").unwrap_or_default(),
                    client_secret: get_str(o, "client_secret").unwrap_or_default().into(),
                });
            }
            if let Some(o) = get_map(m, "oidc") {
                h.oidc = Some(Oidc {
                    issuer_url: get_str(o, "issuer_url").unwrap_or_default(),
                    client_id: get_str(o, "client_id").unwrap_or_default(),
                    client_secret: get_str(o, "client_secret").unwrap_or_default().into(),
                    allow_emails: get_list(o, "allow_emails"),
                    allow_domains: get_list(o, "allow_domains"),
                    scopes: get_list(o, "scopes"),
                });
            }
            if let Some(w) = get_map(m, "verify_webhook") {
                h.webhook_verification = Some(WebhookVerification {
                    provider: get_str(w, "provider").unwrap_or_default(),
                    secret: get_str(w, "secret").unwrap_or_default().into(),
                });
            }
            for (key, add, remove) in [
                ("request_header", &mut h.request_headers_add, &mut h.request_headers_remove),
                ("response_header", &mut h.response_headers_add, &mut h.response_headers_remove),
            ] {
                if let Some(hm) = get_map(m, key) {
                    add.extend(get_list(hm, "add").iter().filter_map(|s| HeaderPair::parse(s)));
                    remove.extend(get_list(hm, "remove"));
                }
            }
            if let Some(ua) = get_map(m, "user_agent_filter") {
                h.allow_user_agents = get_list(ua, "allow");
                h.deny_user_agents = get_list(ua, "deny");
            }
            h.host_header_rewrite = get_str(m, "host_header").as_deref() == Some("rewrite");
            if let Some(hh) = get_str(m, "host_header") {
                if hh != "rewrite" && hh != "preserve" {
                    h.request_headers_add.push(HeaderPair { name: "host".into(), value: hh });
                }
            }
            h.compression = get_bool(m, "compression").unwrap_or(false);
            h.websocket_tcp_conversion = get_bool(m, "websocket_tcp_converter").unwrap_or(false);
            h.circuit_breaker = get_str(m, "circuit_breaker").and_then(|s| s.parse().ok());
            h.mutual_tls_ca_path = get_str(m, "mutual_tls_cas");
            let schemes = get_list(m, "schemes");
            if schemes.len() == 1 {
                h.scheme = Some(schemes[0].clone());
            } else if schemes.len() > 1 {
                warnings.push(format!("tunnel `{name}`: multiple schemes are not supported, using https"));
            }
            if let Some(proto) = get_str(m, "app_protocol") {
                h.app_protocol = Some(proto);
            }
        }
        ProtocolOptions::Tcp(_) => {}
        ProtocolOptions::Tls(t) => {
            t.cert_path = get_str(m, "crt");
            t.key_path = get_str(m, "key");
            t.mutual_tls_ca_path = get_str(m, "mutual_tls_cas");
        }
    }
    for unsupported in ["labels", "bind_tls"] {
        if get(m, unsupported).is_some() {
            warnings.push(format!("tunnel `{name}`: `{unsupported}` is not supported and was ignored"));
        }
    }
    Some(p)
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ExportResult {
    pub yaml: String,
    pub warnings: Vec<String>,
}

/// Render a v3 ngrok.yml for one account and the given profiles.
pub fn export(account: &Account, profiles: &[TunnelProfile], include_secrets: bool) -> ExportResult {
    let mut warnings = Vec::new();
    let mut agent = Map::new();
    if include_secrets {
        if !account.authtoken.is_empty() {
            agent.insert("authtoken".into(), json!(account.authtoken.expose()));
        }
        if !account.api_key.is_empty() {
            agent.insert("api_key".into(), json!(account.api_key.expose()));
        }
    }
    if let Some(s) = &account.server_addr {
        agent.insert("connect_url".into(), json!(s));
    }
    if let Some(s) = &account.proxy_url {
        agent.insert("proxy_url".into(), json!(s));
    }
    if let Some(s) = &account.root_cas {
        agent.insert("root_cas".into(), json!(s));
    }

    let mut endpoints = Vec::new();
    for p in profiles {
        endpoints.push(export_endpoint(p, include_secrets, &mut warnings));
    }
    let doc = json!({ "version": 3, "agent": agent, "endpoints": endpoints });
    let yaml = serde_yaml::to_string(&doc).unwrap_or_default();
    ExportResult { yaml, warnings }
}

fn export_endpoint(p: &TunnelProfile, include_secrets: bool, warnings: &mut Vec<String>) -> Json {
    let mut ep = Map::new();
    ep.insert("name".into(), json!(p.name));
    if !p.url.is_empty() {
        let url = match p.protocol() {
            Protocol::Http if !p.url.contains("://") => format!("https://{}", p.url),
            Protocol::Tcp if !p.url.starts_with("tcp://") => format!("tcp://{}", p.url),
            Protocol::Tls if !p.url.starts_with("tls://") => format!("tls://{}", p.url),
            _ => p.url.clone(),
        };
        ep.insert("url".into(), json!(url));
    } else if p.protocol() != Protocol::Http {
        ep.insert("url".into(), json!(format!("{}://", p.protocol().as_str())));
    }
    if !p.description.is_empty() {
        ep.insert("description".into(), json!(p.description));
    }
    if !p.metadata.is_empty() {
        ep.insert("metadata".into(), json!(p.metadata));
    }
    if p.binding != Binding::Public {
        ep.insert("bindings".into(), json!([p.binding.as_str()]));
    }
    if p.pooling {
        ep.insert("pooling_enabled".into(), json!(true));
    }
    let mut upstream = Map::new();
    upstream.insert("url".into(), json!(p.upstream));
    match p.proxy_proto {
        ProxyProtoVersion::V1 => {
            upstream.insert("proxy_protocol".into(), json!(1));
        }
        ProxyProtoVersion::V2 => {
            upstream.insert("proxy_protocol".into(), json!(2));
        }
        ProxyProtoVersion::None => {}
    }
    if let ProtocolOptions::Http(h) = &p.options {
        if let Some(ap) = &h.app_protocol {
            upstream.insert("protocol".into(), json!(ap));
        }
    }
    ep.insert("upstream".into(), Json::Object(upstream));

    let policy = build_policy(p, include_secrets, warnings);
    if !policy.is_empty() {
        ep.insert("traffic_policy".into(), Json::Object(policy));
    }
    Json::Object(ep)
}

fn push_rule(policy: &mut Map<String, Json>, phase: &str, rule: Json) {
    policy.entry(phase).or_insert_with(|| Json::Array(Vec::new())).as_array_mut().expect("phase is an array").push(rule);
}

/// The profile's own traffic policy plus generated rules for options that
/// have no dedicated v3 field.
fn build_policy(p: &TunnelProfile, include_secrets: bool, warnings: &mut Vec<String>) -> Map<String, Json> {
    let mut generated = Map::new();
    let secret = |s: &Secret, what: &str| -> String {
        if include_secrets {
            s.expose().to_string()
        } else {
            format!("<{what}>")
        }
    };
    let first_phase = if p.protocol() == Protocol::Http { "on_http_request" } else { "on_tcp_connect" };

    if !p.allow_cidrs.is_empty() || !p.deny_cidrs.is_empty() {
        let mut cfg = Map::new();
        if !p.allow_cidrs.is_empty() {
            cfg.insert("allow".into(), json!(p.allow_cidrs));
        }
        if !p.deny_cidrs.is_empty() {
            cfg.insert("deny".into(), json!(p.deny_cidrs));
        }
        push_rule(&mut generated, first_phase, json!({ "actions": [{ "type": "restrict-ips", "config": cfg }] }));
    }

    if let ProtocolOptions::Http(h) = &p.options {
        if !h.basic_auth.is_empty() {
            let creds: Vec<String> = h.basic_auth.iter().map(|b| format!("{}:{}", b.username, secret(&b.password, "password"))).collect();
            push_rule(
                &mut generated,
                "on_http_request",
                json!({ "actions": [{ "type": "basic-auth", "config": { "credentials": creds } }] }),
            );
        }
        if let Some(o) = &h.oauth {
            let mut cfg = Map::new();
            cfg.insert("provider".into(), json!(o.provider));
            if !o.client_id.is_empty() {
                cfg.insert("client_id".into(), json!(o.client_id));
                cfg.insert("client_secret".into(), json!(secret(&o.client_secret, "client_secret")));
            }
            if !o.scopes.is_empty() {
                cfg.insert("scopes".into(), json!(o.scopes));
            }
            push_rule(&mut generated, "on_http_request", json!({ "actions": [{ "type": "oauth", "config": cfg }] }));
            if let Some(expr) = identity_filter("actions.ngrok.oauth.identity.email", &o.allow_emails, &o.allow_domains) {
                push_rule(&mut generated, "on_http_request", json!({ "expressions": [expr], "actions": [{ "type": "deny" }] }));
            }
        }
        if let Some(o) = &h.oidc {
            let mut cfg = Map::new();
            cfg.insert("issuer_url".into(), json!(o.issuer_url));
            cfg.insert("client_id".into(), json!(o.client_id));
            cfg.insert("client_secret".into(), json!(secret(&o.client_secret, "client_secret")));
            if !o.scopes.is_empty() {
                cfg.insert("scopes".into(), json!(o.scopes));
            }
            push_rule(&mut generated, "on_http_request", json!({ "actions": [{ "type": "openid-connect", "config": cfg }] }));
            if let Some(expr) = identity_filter("actions.ngrok.oidc.identity.email", &o.allow_emails, &o.allow_domains) {
                push_rule(&mut generated, "on_http_request", json!({ "expressions": [expr], "actions": [{ "type": "deny" }] }));
            }
        }
        if let Some(w) = &h.webhook_verification {
            push_rule(
                &mut generated,
                "on_http_request",
                json!({ "actions": [{ "type": "verify-webhook", "config": { "provider": w.provider, "secret": secret(&w.secret, "secret") } }] }),
            );
        }
        if !h.deny_user_agents.is_empty() || !h.allow_user_agents.is_empty() {
            let mut conds = Vec::new();
            for re in &h.deny_user_agents {
                conds.push(format!("req.user_agent.raw.matches('{}')", re.replace('\'', "\\'")));
            }
            if !conds.is_empty() {
                push_rule(
                    &mut generated,
                    "on_http_request",
                    json!({ "expressions": [conds.join(" || ")], "actions": [{ "type": "deny" }] }),
                );
            }
            if !h.allow_user_agents.is_empty() {
                let allow: Vec<String> =
                    h.allow_user_agents.iter().map(|re| format!("!req.user_agent.raw.matches('{}')", re.replace('\'', "\\'"))).collect();
                push_rule(
                    &mut generated,
                    "on_http_request",
                    json!({ "expressions": [allow.join(" && ")], "actions": [{ "type": "deny" }] }),
                );
            }
        }
        if let Some(cb) = h.circuit_breaker {
            push_rule(
                &mut generated,
                "on_http_request",
                json!({ "actions": [{ "type": "circuit-breaker", "config": { "error_threshold": cb } }] }),
            );
        }
        let mut req_add: Map<String, Json> = h.request_headers_add.iter().map(|hp| (hp.name.clone(), json!(hp.value))).collect();
        if h.host_header_rewrite {
            warnings.push(format!("`{}`: host header rewrite exported as a static Host header", p.name));
            if let Ok(u) = crate::upstream::normalize_upstream(&p.upstream, Protocol::Http) {
                if let Some(host) = u.host_str() {
                    req_add.insert("host".into(), json!(host));
                }
            }
        }
        if !h.request_headers_remove.is_empty() {
            push_rule(
                &mut generated,
                "on_http_request",
                json!({ "actions": [{ "type": "remove-headers", "config": { "headers": h.request_headers_remove } }] }),
            );
        }
        if !req_add.is_empty() {
            push_rule(
                &mut generated,
                "on_http_request",
                json!({ "actions": [{ "type": "add-headers", "config": { "headers": req_add } }] }),
            );
        }
        if !h.response_headers_remove.is_empty() {
            push_rule(
                &mut generated,
                "on_http_response",
                json!({ "actions": [{ "type": "remove-headers", "config": { "headers": h.response_headers_remove } }] }),
            );
        }
        if !h.response_headers_add.is_empty() {
            let add: Map<String, Json> = h.response_headers_add.iter().map(|hp| (hp.name.clone(), json!(hp.value))).collect();
            push_rule(&mut generated, "on_http_response", json!({ "actions": [{ "type": "add-headers", "config": { "headers": add } }] }));
        }
        if h.compression {
            push_rule(&mut generated, "on_http_response", json!({ "actions": [{ "type": "compress-response" }] }));
        }
        if h.mutual_tls_ca_path.is_some() {
            warnings.push(format!("`{}`: mutual TLS CAs cannot be exported to ngrok.yml v3, configure terminate-tls manually", p.name));
        }
        if h.websocket_tcp_conversion {
            warnings.push(format!("`{}`: websocket-tcp conversion has no v3 equivalent and was dropped", p.name));
        }
    }
    if let ProtocolOptions::Tls(t) = &p.options {
        if t.cert_path.is_some() {
            warnings.push(format!("`{}`: TLS termination certificates must be configured with a terminate-tls action", p.name));
        }
    }

    // Merge: generated rules first (auth/IP gating), then the user's own policy.
    if let Ok(Some(user_json)) = crate::upstream::traffic_policy_to_json(&p.traffic_policy) {
        if let Ok(Json::Object(user)) = serde_json::from_str::<Json>(&user_json) {
            for (phase, rules) in user {
                if let Json::Array(rules) = rules {
                    for r in rules {
                        push_rule(&mut generated, &phase, r);
                    }
                }
            }
        }
    } else if !p.traffic_policy.trim().is_empty() {
        warnings.push(format!("`{}`: traffic policy is invalid and was not exported", p.name));
    }
    generated
}

fn identity_filter(field: &str, emails: &[String], domains: &[String]) -> Option<String> {
    if emails.is_empty() && domains.is_empty() {
        return None;
    }
    let mut allowed = Vec::new();
    for e in emails {
        allowed.push(format!("{field} == '{e}'"));
    }
    for d in domains {
        allowed.push(format!("{field}.endsWith('@{d}')"));
    }
    Some(format!("!({})", allowed.join(" || ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2: &str = r#"
version: "2"
authtoken: tok_v2
api_key: key_v2
region: eu
tunnels:
  web:
    proto: http
    addr: 3000
    domain: foo.ngrok.app
    basic_auth: ["alice:password123"]
    ip_restriction:
      allow_cidrs: [10.0.0.0/8]
    request_header:
      add: ["X-Env: dev"]
      remove: [Cookie]
    host_header: rewrite
    compression: true
    oauth:
      provider: google
      allow_domains: [example.com]
  ssh:
    proto: tcp
    addr: 22
    remote_addr: 1.tcp.ngrok.io:12345
  weird:
    proto: ftp
    addr: 21
"#;

    const V3: &str = r#"
version: 3
agent:
  authtoken: tok_v3
  connect_url: eu.connect.ngrok-agent.com:443
endpoints:
  - name: api
    url: https://api.example.com
    upstream:
      url: 8080
      protocol: http2
    traffic_policy:
      on_http_request:
        - actions:
            - type: rate-limit
              config: { capacity: 10, rate: 60s, algorithm: sliding_window, bucket_key: [conn.client_ip] }
  - name: db
    url: tcp://5.tcp.ngrok.io:2000
    upstream:
      url: localhost:5432
      proxy_protocol: 2
    pooling_enabled: true
"#;

    #[test]
    fn imports_v2() {
        let acc = Uuid::new_v4();
        let r = import(V2, acc).unwrap();
        assert_eq!(r.authtoken.as_deref(), Some("tok_v2"));
        assert_eq!(r.api_key.as_deref(), Some("key_v2"));
        assert_eq!(r.profiles.len(), 2);
        assert!(r.warnings.iter().any(|w| w.contains("region")));
        assert!(r.warnings.iter().any(|w| w.contains("ftp")));
        let web = r.profiles.iter().find(|p| p.name == "web").unwrap();
        assert_eq!(web.url, "foo.ngrok.app");
        assert_eq!(web.allow_cidrs, vec!["10.0.0.0/8"]);
        let ProtocolOptions::Http(h) = &web.options else { panic!() };
        assert_eq!(h.basic_auth[0].password.expose(), "password123");
        assert!(h.host_header_rewrite && h.compression);
        assert_eq!(h.request_headers_add[0].name, "X-Env");
        assert_eq!(h.oauth.as_ref().unwrap().allow_domains, vec!["example.com"]);
        let ssh = r.profiles.iter().find(|p| p.name == "ssh").unwrap();
        assert_eq!(ssh.protocol(), Protocol::Tcp);
        assert_eq!(ssh.url, "1.tcp.ngrok.io:12345");
    }

    #[test]
    fn imports_v3() {
        let r = import(V3, Uuid::new_v4()).unwrap();
        assert_eq!(r.authtoken.as_deref(), Some("tok_v3"));
        assert_eq!(r.server_addr.as_deref(), Some("eu.connect.ngrok-agent.com:443"));
        let api = &r.profiles[0];
        assert_eq!(api.upstream, "8080");
        assert!(api.traffic_policy.contains("rate-limit"));
        assert!(crate::upstream::traffic_policy_to_json(&api.traffic_policy).unwrap().is_some());
        let ProtocolOptions::Http(h) = &api.options else { panic!() };
        assert_eq!(h.app_protocol.as_deref(), Some("http2"));
        let db = &r.profiles[1];
        assert_eq!(db.protocol(), Protocol::Tcp);
        assert_eq!(db.url, "5.tcp.ngrok.io:2000");
        assert_eq!(db.proxy_proto, ProxyProtoVersion::V2);
        assert!(db.pooling);
    }

    #[test]
    fn export_roundtrip_v3() {
        let acc_id = Uuid::new_v4();
        let r = import(V2, acc_id).unwrap();
        let mut acc = Account::new("x");
        acc.id = acc_id;
        acc.authtoken = "tok".into();
        let without = export(&acc, &r.profiles, false);
        assert!(!without.yaml.contains("tok"));
        assert!(!without.yaml.contains("password123"));
        assert!(without.yaml.contains("<password>"));
        let with = export(&acc, &r.profiles, true);
        assert!(with.yaml.contains("alice:password123"));
        assert!(with.yaml.contains("restrict-ips"));
        assert!(with.yaml.contains("compress-response"));

        let back = import(&with.yaml, acc_id).unwrap();
        assert_eq!(back.authtoken.as_deref(), Some("tok"));
        assert_eq!(back.profiles.len(), 2);
        let web = back.profiles.iter().find(|p| p.name == "web").unwrap();
        assert_eq!(web.url, "https://foo.ngrok.app");
        let json = crate::upstream::traffic_policy_to_json(&web.traffic_policy).unwrap().unwrap();
        assert!(json.contains("basic-auth"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(import("- a\n- b\n", Uuid::nil()), Err(ImportError::NotAMapping)));
        assert!(matches!(import("version: 9\n", Uuid::nil()), Err(ImportError::Version(_))));
    }
}
