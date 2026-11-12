//! Tunnel profile editor: every option of `ngrok http|tcp|tls`.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSColor, NSPopUpButton, NSSecureTextField, NSStackView, NSTextField, NSTextView, NSView, NSWindow};
use shaft_config::{
    validate_profile, BasicAuth, Binding, HeaderPair, HttpOptions, OAuth, Oidc, ProfileHotkey, Protocol, ProtocolOptions,
    ProxyProtoVersion, TlsOptions, TunnelProfile, WebhookVerification,
};
use uuid::Uuid;

use super::{join_list, split_list};
use crate::{
    app::app,
    ui::{self, ns, Handlers},
};

const OAUTH_PROVIDERS: [&str; 9] = ["(none)", "google", "github", "gitlab", "microsoft", "linkedin", "twitch", "amazon", "facebook"];
const PROXY_PROTO: [&str; 3] = ["off", "v1", "v2"];
const APP_PROTO: [&str; 2] = ["http1", "http2"];
const SCHEMES: [&str; 2] = ["https", "http"];

pub struct ProfileWindow {
    pub window: Retained<NSWindow>,
    _handlers: Handlers,
    base: TunnelProfile,
    is_new: bool,
    account_ids: Vec<Uuid>,
    // General
    name: Retained<NSTextField>,
    account: Retained<NSPopUpButton>,
    protocol: Retained<NSPopUpButton>,
    upstream: Retained<NSTextField>,
    url: Retained<NSTextField>,
    favorite: Retained<NSButton>,
    inspect: Retained<NSButton>,
    hotkey: Retained<NSTextField>,
    // Common
    description: Retained<NSTextField>,
    metadata: Retained<NSTextField>,
    binding: Retained<NSPopUpButton>,
    pooling: Retained<NSButton>,
    proxy_proto: Retained<NSPopUpButton>,
    verify_tls: Retained<NSButton>,
    allow_cidrs: Retained<NSTextField>,
    deny_cidrs: Retained<NSTextField>,
    policy: Retained<NSTextView>,
    // HTTP
    http_section: Retained<NSStackView>,
    scheme: Retained<NSPopUpButton>,
    basic_auth: Retained<NSTextView>,
    oauth_provider: Retained<NSPopUpButton>,
    oauth_emails: Retained<NSTextField>,
    oauth_domains: Retained<NSTextField>,
    oauth_scopes: Retained<NSTextField>,
    oauth_client_id: Retained<NSTextField>,
    oauth_client_secret: Retained<NSSecureTextField>,
    oidc_issuer: Retained<NSTextField>,
    oidc_client_id: Retained<NSTextField>,
    oidc_client_secret: Retained<NSSecureTextField>,
    oidc_emails: Retained<NSTextField>,
    oidc_domains: Retained<NSTextField>,
    oidc_scopes: Retained<NSTextField>,
    webhook_provider: Retained<NSTextField>,
    webhook_secret: Retained<NSSecureTextField>,
    req_add: Retained<NSTextView>,
    req_remove: Retained<NSTextField>,
    resp_add: Retained<NSTextView>,
    resp_remove: Retained<NSTextField>,
    host_rewrite: Retained<NSButton>,
    compression: Retained<NSButton>,
    circuit_breaker: Retained<NSTextField>,
    ws_tcp: Retained<NSButton>,
    app_protocol: Retained<NSPopUpButton>,
    http_mtls: Retained<NSTextField>,
    ua_allow: Retained<NSTextField>,
    ua_deny: Retained<NSTextField>,
    // TLS
    tls_section: Retained<NSStackView>,
    tls_cert: Retained<NSTextField>,
    tls_key: Retained<NSTextField>,
    tls_mtls: Retained<NSTextField>,
    errors: Retained<NSTextField>,
}

/// Open the editor for an existing profile, or a new one.
pub fn open(profile_id: Option<Uuid>) {
    let app = app();
    app.close_popover();
    let cfg = app.config.borrow().clone();
    if cfg.accounts.is_empty() {
        ui::error("No account", "Add an ngrok account before creating profiles.");
        return;
    }
    let (base, is_new) = match profile_id.and_then(|id| cfg.profile(id).cloned()) {
        Some(p) => (p, false),
        None => {
            let acc = cfg.active_account().map(|a| a.id).unwrap_or(cfg.accounts[0].id);
            let mut p = TunnelProfile::new("", acc, Protocol::Http, "");
            // Pre-fill from the quick tunnel form.
            let q = app.quick.borrow().clone();
            p.upstream = q.upstream;
            p.options = ProtocolOptions::for_protocol(q.protocol);
            (p, true)
        }
    };
    if let Some(old) = app.windows.profile.borrow_mut().take() {
        old.window.close();
    }
    let w = Rc::new(build(base, is_new, &cfg));
    ui::show_window(&w.window);
    *app.windows.profile.borrow_mut() = Some(w);
}

fn build(base: TunnelProfile, is_new: bool, cfg: &shaft_config::AppConfig) -> ProfileWindow {
    let h = Handlers::new();
    let title = if is_new { "New Tunnel Profile".to_string() } else { format!("Edit “{}”", base.name) };
    let window = ui::window(&title, 640.0, 720.0, true);

    let account_ids: Vec<Uuid> = cfg.accounts.iter().map(|a| a.id).collect();
    let account_names: Vec<&str> = cfg.accounts.iter().map(|a| a.name.as_str()).collect();
    let protos: Vec<&str> = Protocol::ALL.iter().map(|p| p.label()).collect();
    let hotkey_value =
        cfg.settings.hotkeys.profiles.iter().find(|p| p.profile_id == base.id).map(|p| p.accelerator.clone()).unwrap_or_default();

    let http = match &base.options {
        ProtocolOptions::Http(h) => h.clone(),
        _ => HttpOptions::default(),
    };
    let tls = match &base.options {
        ProtocolOptions::Tls(t) => t.clone(),
        _ => TlsOptions::default(),
    };
    let oauth = http.oauth.clone().unwrap_or_default();
    let oidc = http.oidc.clone().unwrap_or_default();
    let webhook = http.webhook_verification.clone().unwrap_or_default();

    // --- General
    let name = ui::text_field(&base.name, "my-api");
    let account = ui::popup(&account_names, account_ids.iter().position(|i| *i == base.account_id).unwrap_or(0));
    let protocol = ui::popup(&protos, Protocol::ALL.iter().position(|p| *p == base.protocol()).unwrap_or(0));
    let upstream = ui::text_field(&base.upstream, "3000, localhost:8080, https://local.test:8443, unix:///tmp/app.sock");
    let url = ui::text_field(&base.url, "");
    let favorite = ui::checkbox("Favorite (pinned in the menu)", base.favorite);
    let inspect = ui::checkbox("Capture traffic in the Inspector (HTTP)", base.inspect);
    let hotkey = ui::text_field(&hotkey_value, "e.g. cmd+shift+1 (optional)");

    // --- Common advanced
    let description = ui::text_field(&base.description, "Shown as “forwards to” in the dashboard");
    let metadata = ui::text_field(&base.metadata, "Arbitrary string");
    let binding = ui::popup(&Binding::ALL.map(|b| b.as_str()), Binding::ALL.iter().position(|b| *b == base.binding).unwrap_or(0));
    let pooling = ui::checkbox("Pooling (load-balance with other agents on the same URL)", base.pooling);
    let proxy_proto = ui::popup(&PROXY_PROTO, base.proxy_proto as usize);
    let verify_tls = ui::checkbox("Verify upstream TLS certificate", base.verify_upstream_tls);
    let allow_cidrs = ui::text_field(&join_list(&base.allow_cidrs), "10.0.0.0/8, 203.0.113.4/32");
    let deny_cidrs = ui::text_field(&join_list(&base.deny_cidrs), "");
    let (policy_scroll, policy) = ui::text_area(&base.traffic_policy, true, true);
    ui::set_height(&policy_scroll, 140.0);

    // --- HTTP
    let scheme = ui::popup(&SCHEMES, if http.scheme.as_deref() == Some("http") { 1 } else { 0 });
    let ba_text: Vec<String> = http.basic_auth.iter().map(|b| format!("{}:{}", b.username, b.password.expose())).collect();
    let (ba_scroll, basic_auth) = ui::text_area(&ba_text.join("\n"), true, true);
    ui::set_height(&ba_scroll, 48.0);
    let oauth_provider = ui::popup(&OAUTH_PROVIDERS, OAUTH_PROVIDERS.iter().position(|p| *p == oauth.provider).unwrap_or(0));
    let oauth_emails = ui::text_field(&join_list(&oauth.allow_emails), "allowed emails");
    let oauth_domains = ui::text_field(&join_list(&oauth.allow_domains), "allowed domains");
    let oauth_scopes = ui::text_field(&join_list(&oauth.scopes), "extra scopes");
    let oauth_client_id = ui::text_field(&oauth.client_id, "custom OAuth app (optional)");
    let oauth_client_secret = ui::secure_field(oauth.client_secret.expose(), "client secret");
    let oidc_issuer = ui::text_field(&oidc.issuer_url, "https://accounts.example.com");
    let oidc_client_id = ui::text_field(&oidc.client_id, "client id");
    let oidc_client_secret = ui::secure_field(oidc.client_secret.expose(), "client secret");
    let oidc_emails = ui::text_field(&join_list(&oidc.allow_emails), "allowed emails");
    let oidc_domains = ui::text_field(&join_list(&oidc.allow_domains), "allowed domains");
    let oidc_scopes = ui::text_field(&join_list(&oidc.scopes), "scopes");
    let webhook_provider = ui::text_field(&webhook.provider, "github, stripe, slack, twilio, shopify…");
    let webhook_secret = ui::secure_field(webhook.secret.expose(), "signing secret");
    let req_add_text: Vec<String> = http.request_headers_add.iter().map(|h| h.to_string()).collect();
    let (req_add_scroll, req_add) = ui::text_area(&req_add_text.join("\n"), true, true);
    ui::set_height(&req_add_scroll, 48.0);
    let req_remove = ui::text_field(&join_list(&http.request_headers_remove), "Cookie, X-Debug");
    let resp_add_text: Vec<String> = http.response_headers_add.iter().map(|h| h.to_string()).collect();
    let (resp_add_scroll, resp_add) = ui::text_area(&resp_add_text.join("\n"), true, true);
    ui::set_height(&resp_add_scroll, 48.0);
    let resp_remove = ui::text_field(&join_list(&http.response_headers_remove), "Server");
    let host_rewrite = ui::checkbox("Rewrite Host header to the upstream host", http.host_header_rewrite);
    let compression = ui::checkbox("gzip-compress responses", http.compression);
    let circuit_breaker = ui::text_field(&http.circuit_breaker.map(|c| c.to_string()).unwrap_or_default(), "0.5 = trip at 50% 5xx");
    let ws_tcp = ui::checkbox("Convert WebSockets to TCP streams", http.websocket_tcp_conversion);
    let app_protocol = ui::popup(&APP_PROTO, if http.app_protocol.as_deref() == Some("http2") { 1 } else { 0 });
    let http_mtls = ui::text_field(http.mutual_tls_ca_path.as_deref().unwrap_or(""), "CA bundle PEM path (mutual TLS)");
    let ua_allow = ui::text_field(&join_list(&http.allow_user_agents), "regexes");
    let ua_deny = ui::text_field(&join_list(&http.deny_user_agents), "regexes");

    // --- TLS
    let tls_cert = ui::text_field(tls.cert_path.as_deref().unwrap_or(""), "certificate PEM (terminate at edge)");
    let tls_key = ui::text_field(tls.key_path.as_deref().unwrap_or(""), "private key PEM");
    let tls_mtls = ui::text_field(tls.mutual_tls_ca_path.as_deref().unwrap_or(""), "CA bundle PEM (mutual TLS)");

    let browse = |field: &Retained<NSTextField>| {
        let f = field.clone();
        ui::small_button("Browse…", &h, move || {
            if let Some(p) = ui::open_panel(None) {
                f.setStringValue(&ns(&p.to_string_lossy()));
            }
        })
    };
    let http_mtls_row = ui::fill_hstack(&[&http_mtls, &browse(&http_mtls)], 6.0);
    let tls_cert_row = ui::fill_hstack(&[&tls_cert, &browse(&tls_cert)], 6.0);
    let tls_key_row = ui::fill_hstack(&[&tls_key, &browse(&tls_key)], 6.0);
    let tls_mtls_row = ui::fill_hstack(&[&tls_mtls, &browse(&tls_mtls)], 6.0);

    const LW: f64 = 150.0;
    let general = ui::form(
        &[
            ("Name", &name),
            ("Account", &account),
            ("Protocol", &protocol),
            ("Upstream", &upstream),
            ("Public URL / domain", &url),
            ("", &favorite),
            ("", &inspect),
            ("Global shortcut", &hotkey),
        ],
        LW,
    );
    let common = ui::form(
        &[
            ("Description", &description),
            ("Metadata", &metadata),
            ("Binding", &binding),
            ("", &pooling),
            ("PROXY protocol", &proxy_proto),
            ("", &verify_tls),
            ("Allow CIDRs", &allow_cidrs),
            ("Deny CIDRs", &deny_cidrs),
        ],
        LW,
    );
    let http_form = ui::form(
        &[
            ("Scheme", &scheme),
            ("Basic auth (user:pass)", &ba_scroll),
            ("OAuth provider", &oauth_provider),
            ("OAuth emails", &oauth_emails),
            ("OAuth domains", &oauth_domains),
            ("OAuth scopes", &oauth_scopes),
            ("OAuth client id", &oauth_client_id),
            ("OAuth client secret", &oauth_client_secret),
            ("OIDC issuer", &oidc_issuer),
            ("OIDC client id", &oidc_client_id),
            ("OIDC client secret", &oidc_client_secret),
            ("OIDC emails", &oidc_emails),
            ("OIDC domains", &oidc_domains),
            ("OIDC scopes", &oidc_scopes),
            ("Webhook provider", &webhook_provider),
            ("Webhook secret", &webhook_secret),
            ("Add request headers", &req_add_scroll),
            ("Remove request headers", &req_remove),
            ("Add response headers", &resp_add_scroll),
            ("Remove response headers", &resp_remove),
            ("", &host_rewrite),
            ("", &compression),
            ("Circuit breaker", &circuit_breaker),
            ("", &ws_tcp),
            ("Upstream protocol", &app_protocol),
            ("Mutual TLS CAs", &http_mtls_row),
            ("Allow user agents", &ua_allow),
            ("Deny user agents", &ua_deny),
        ],
        LW,
    );
    let http_section = ui::vstack(&[&ui::section_label("HTTP options"), &http_form], 8.0);
    ui::fill_width(&http_section, &http_form);
    let tls_form = ui::form(&[("Certificate", &tls_cert_row), ("Private key", &tls_key_row), ("Mutual TLS CAs", &tls_mtls_row)], LW);
    let tls_section = ui::vstack(&[&ui::section_label("TLS options"), &tls_form], 8.0);
    ui::fill_width(&tls_section, &tls_form);

    let policy_help = ui::secondary_label("Traffic Policy (YAML or JSON) — https://ngrok.com/docs/traffic-policy");
    let errors = ui::wrapping_label("", 580.0);
    errors.setTextColor(Some(&NSColor::systemRedColor()));

    let content = ui::vstack(
        &[
            &ui::section_label("General"),
            &general,
            &ui::separator(),
            &ui::section_label("Endpoint"),
            &common,
            &ui::separator(),
            &http_section,
            &tls_section,
            &ui::separator(),
            &policy_help,
            &policy_scroll,
        ],
        10.0,
    );
    for v in [&*general as &NSView, &common, &http_section, &tls_section, &policy_scroll] {
        ui::fill_width(&content, v);
    }
    let scroll = ui::scroll_stack(&content);
    ui::set_min_height(&scroll, 420.0);

    let save = ui::button("Save", &h, || with(|w| w.save(false)));
    save.setKeyEquivalent(&ns("\r"));
    let save_start = ui::button("Save & Start", &h, || with(|w| w.save(true)));
    let cancel = ui::button("Cancel", &h, || with(|w| w.window.close()));
    let delete = ui::button("Delete…", &h, || with(|w| w.delete()));
    delete.setHidden(is_new);
    let export = ui::button("Export YAML…", &h, || with(|w| w.export()));
    let buttons = ui::hstack(&[&delete, &export, &ui::spacer(), &cancel, &save_start, &save], 8.0);

    let root = ui::vstack(&[&scroll, &errors, &buttons], 10.0);
    for v in [&*scroll as &NSView, &errors, &buttons] {
        ui::fill_width(&root, v);
    }
    ui::set_window_content(&window, &root);

    let w = ProfileWindow {
        window,
        _handlers: h,
        base,
        is_new,
        account_ids,
        name,
        account,
        protocol,
        upstream,
        url,
        favorite,
        inspect,
        hotkey,
        description,
        metadata,
        binding,
        pooling,
        proxy_proto,
        verify_tls,
        allow_cidrs,
        deny_cidrs,
        policy,
        http_section,
        scheme,
        basic_auth,
        oauth_provider,
        oauth_emails,
        oauth_domains,
        oauth_scopes,
        oauth_client_id,
        oauth_client_secret,
        oidc_issuer,
        oidc_client_id,
        oidc_client_secret,
        oidc_emails,
        oidc_domains,
        oidc_scopes,
        webhook_provider,
        webhook_secret,
        req_add,
        req_remove,
        resp_add,
        resp_remove,
        host_rewrite,
        compression,
        circuit_breaker,
        ws_tcp,
        app_protocol,
        http_mtls,
        ua_allow,
        ua_deny,
        tls_section,
        tls_cert,
        tls_key,
        tls_mtls,
        errors,
    };
    ui::on_action(&w.protocol, &w._handlers, || with(|w| w.protocol_changed()));
    w.protocol_changed();
    w
}

fn with(f: impl FnOnce(&ProfileWindow)) {
    let w = app().windows.profile.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn opt(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

impl ProfileWindow {
    fn current_protocol(&self) -> Protocol {
        Protocol::ALL[ui::selected(&self.protocol).min(2)]
    }

    fn protocol_changed(&self) {
        let p = self.current_protocol();
        self.http_section.setHidden(p != Protocol::Http);
        self.tls_section.setHidden(p != Protocol::Tls);
        self.inspect.setEnabled(p == Protocol::Http);
        let placeholder = match p {
            Protocol::Http => "my-app.ngrok.app or https://api.example.com (empty = random)",
            Protocol::Tcp => "reserved address, e.g. 1.tcp.ngrok.io:12345 (empty = random)",
            Protocol::Tls => "domain, e.g. secure.example.com",
        };
        self.url.setPlaceholderString(Some(&ns(placeholder)));
    }

    fn collect(&self) -> Result<TunnelProfile, Vec<String>> {
        let mut errs = Vec::new();
        let mut p = self.base.clone();
        p.name = ui::value(&self.name);
        p.account_id = self.account_ids.get(ui::selected(&self.account)).copied().unwrap_or(p.account_id);
        p.upstream = ui::value(&self.upstream);
        p.url = ui::value(&self.url);
        p.favorite = ui::is_checked(&self.favorite);
        p.inspect = ui::is_checked(&self.inspect);
        p.description = ui::value(&self.description);
        p.metadata = ui::value(&self.metadata);
        p.binding = Binding::ALL[ui::selected(&self.binding).min(2)];
        p.pooling = ui::is_checked(&self.pooling);
        p.proxy_proto = [ProxyProtoVersion::None, ProxyProtoVersion::V1, ProxyProtoVersion::V2][ui::selected(&self.proxy_proto).min(2)];
        p.verify_upstream_tls = ui::is_checked(&self.verify_tls);
        p.allow_cidrs = split_list(&ui::value(&self.allow_cidrs));
        p.deny_cidrs = split_list(&ui::value(&self.deny_cidrs));
        p.traffic_policy = ui::text_of(&self.policy);

        p.options = match self.current_protocol() {
            Protocol::Http => {
                let mut h = HttpOptions { scheme: Some(SCHEMES[ui::selected(&self.scheme).min(1)].into()), ..Default::default() };
                if h.scheme.as_deref() == Some("https") {
                    h.scheme = None;
                }
                for line in ui::text_of(&self.basic_auth).lines().map(str::trim).filter(|l| !l.is_empty()) {
                    match line.split_once(':') {
                        Some((u, pw)) => h.basic_auth.push(BasicAuth { username: u.trim().into(), password: pw.into() }),
                        None => errs.push(format!("basic auth line `{line}` must be user:password")),
                    }
                }
                let provider = OAUTH_PROVIDERS[ui::selected(&self.oauth_provider).min(OAUTH_PROVIDERS.len() - 1)];
                if provider != "(none)" {
                    h.oauth = Some(OAuth {
                        provider: provider.into(),
                        allow_emails: split_list(&ui::value(&self.oauth_emails)),
                        allow_domains: split_list(&ui::value(&self.oauth_domains)),
                        scopes: split_list(&ui::value(&self.oauth_scopes)),
                        client_id: ui::value(&self.oauth_client_id),
                        client_secret: self.oauth_client_secret.stringValue().to_string().into(),
                    });
                }
                let issuer = ui::value(&self.oidc_issuer);
                if !issuer.is_empty() {
                    h.oidc = Some(Oidc {
                        issuer_url: issuer,
                        client_id: ui::value(&self.oidc_client_id),
                        client_secret: self.oidc_client_secret.stringValue().to_string().into(),
                        allow_emails: split_list(&ui::value(&self.oidc_emails)),
                        allow_domains: split_list(&ui::value(&self.oidc_domains)),
                        scopes: split_list(&ui::value(&self.oidc_scopes)),
                    });
                }
                let wp = ui::value(&self.webhook_provider);
                if !wp.is_empty() {
                    h.webhook_verification =
                        Some(WebhookVerification { provider: wp, secret: self.webhook_secret.stringValue().to_string().into() });
                }
                for (text, out) in
                    [(ui::text_of(&self.req_add), &mut h.request_headers_add), (ui::text_of(&self.resp_add), &mut h.response_headers_add)]
                {
                    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                        match HeaderPair::parse(line) {
                            Some(hp) => out.push(hp),
                            None => errs.push(format!("header `{line}` must be `Name: value`")),
                        }
                    }
                }
                h.request_headers_remove = split_list(&ui::value(&self.req_remove));
                h.response_headers_remove = split_list(&ui::value(&self.resp_remove));
                h.host_header_rewrite = ui::is_checked(&self.host_rewrite);
                h.compression = ui::is_checked(&self.compression);
                let cb = ui::value(&self.circuit_breaker);
                if !cb.is_empty() {
                    match cb.parse::<f64>() {
                        Ok(v) => h.circuit_breaker = Some(v),
                        Err(_) => errs.push("circuit breaker must be a number between 0 and 1".into()),
                    }
                }
                h.websocket_tcp_conversion = ui::is_checked(&self.ws_tcp);
                h.app_protocol = (ui::selected(&self.app_protocol) == 1).then(|| "http2".to_string());
                h.mutual_tls_ca_path = opt(ui::value(&self.http_mtls));
                h.allow_user_agents = split_list(&ui::value(&self.ua_allow));
                h.deny_user_agents = split_list(&ui::value(&self.ua_deny));
                ProtocolOptions::Http(h)
            }
            Protocol::Tcp => ProtocolOptions::Tcp(Default::default()),
            Protocol::Tls => ProtocolOptions::Tls(TlsOptions {
                cert_path: opt(ui::value(&self.tls_cert)),
                key_path: opt(ui::value(&self.tls_key)),
                mutual_tls_ca_path: opt(ui::value(&self.tls_mtls)),
            }),
        };
        let hk = ui::value(&self.hotkey);
        if !hk.is_empty() {
            if let Err(e) = crate::platform::parse_accelerator(&hk) {
                errs.push(format!("shortcut: {e}"));
            }
        }
        errs.extend(validate_profile(&p));
        // Name must be unique.
        let dup = app().config.borrow().profiles.iter().any(|o| o.id != p.id && o.name.eq_ignore_ascii_case(&p.name));
        if dup {
            errs.push(format!("a profile named `{}` already exists", p.name));
        }
        if errs.is_empty() {
            Ok(p)
        } else {
            Err(errs)
        }
    }

    fn save(&self, start: bool) {
        let p = match self.collect() {
            Ok(p) => p,
            Err(errs) => {
                self.errors.setStringValue(&ns(&errs.join("\n")));
                return;
            }
        };
        self.errors.setStringValue(&ns(""));
        let app = app();
        let running = app.core.running_profile(p.id).is_some();
        {
            let mut cfg = app.config.borrow_mut();
            let hk = ui::value(&self.hotkey);
            cfg.settings.hotkeys.profiles.retain(|x| x.profile_id != p.id);
            if !hk.is_empty() {
                cfg.settings.hotkeys.profiles.push(ProfileHotkey { profile_id: p.id, accelerator: hk });
            }
            cfg.upsert_profile(p.clone());
        }
        if !app.save() {
            return;
        }
        self.window.close();
        if start || running {
            if running {
                app.core.stop_profile(p.id);
                // Restart once the old run is gone.
                let id = p.id;
                crate::platform::on_main(move || restart_when_stopped(id, 0));
            } else {
                app.start_profile(p.id);
            }
        }
    }

    fn delete(&self) {
        if self.is_new {
            return;
        }
        let name = self.base.name.clone();
        if !ui::confirm(
            &format!("Delete “{name}”?"),
            "The profile and its stored secrets will be removed. This cannot be undone.",
            "Delete",
            true,
        ) {
            return;
        }
        let app = app();
        app.core.stop_profile(self.base.id);
        let id = self.base.id;
        {
            let mut cfg = app.config.borrow_mut();
            cfg.profiles.retain(|p| p.id != id);
            cfg.settings.hotkeys.profiles.retain(|x| x.profile_id != id);
        }
        if let Err(e) = app.store.forget_profile_secrets(&self.base) {
            tracing::warn!(error = %e, "could not remove profile secrets");
        }
        app.save();
        self.window.close();
    }

    fn export(&self) {
        let p = match self.collect() {
            Ok(p) => p,
            Err(errs) => {
                self.errors.setStringValue(&ns(&errs.join("\n")));
                return;
            }
        };
        let app = app();
        let Some(acc) = app.config.borrow().account(p.account_id).cloned() else { return };
        super::accounts::export_profiles(&acc, &[p]);
    }
}

fn restart_when_stopped(profile_id: Uuid, tries: u32) {
    let app = app();
    if app.core.running_profile(profile_id).is_none() {
        app.start_profile(profile_id);
    } else if tries < 50 {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            crate::platform::on_main(move || restart_when_stopped(profile_id, tries + 1));
        });
    }
}
