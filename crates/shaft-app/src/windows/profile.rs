//! Tunnels pane: list of saved profiles on the left, a System Settings–style
//! editor on the right with every option of `ngrok http|tcp|tls`.

use std::{cell::RefCell, rc::Rc};

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont, NSPopUpButton, NSSecureTextField, NSStackView, NSSwitch, NSTextField, NSTextView, NSView};
use shaft_config::{
    validate_profile, BasicAuth, Binding, HeaderPair, HttpOptions, OAuth, Oidc, ProfileHotkey, Protocol, ProtocolOptions,
    ProxyProtoVersion, TlsOptions, TunnelProfile, WebhookVerification,
};
use uuid::Uuid;

use super::{join_list, main::Section, split_list};
use crate::{
    app::app,
    kit::{self, weight},
    ui::{self, ns, Handlers, Table},
};

const OAUTH_PROVIDERS: [&str; 9] = ["None", "google", "github", "gitlab", "microsoft", "linkedin", "twitch", "amazon", "facebook"];
const PROXY_PROTO: [&str; 3] = ["Off", "v1", "v2"];
const APP_PROTO: [&str; 2] = ["HTTP/1.1", "HTTP/2"];
const SCHEMES: [&str; 2] = ["HTTPS", "HTTP"];
const LW: f64 = 150.0;
const LIST_W: f64 = 280.0;

pub struct TunnelsPane {
    pub view: Retained<NSView>,
    _h: Handlers,
    table: Table,
    ids: RefCell<Vec<Uuid>>,
    editor_host: Retained<NSView>,
    editor: RefCell<Option<Rc<Editor>>>,
}

/// Show the Tunnels section, editing `profile_id` (or a new profile).
pub fn open(profile_id: Option<Uuid>) {
    if app().config.borrow().accounts.is_empty() {
        ui::error("No account", "Add an ngrok account before creating profiles.");
        return;
    }
    super::main::show(Section::Tunnels);
    with(|p| match profile_id {
        Some(id) => p.select(Some(id)),
        None => p.new_profile(),
    });
}

pub fn pane() -> Retained<NSView> {
    let p = Rc::new(build());
    *app().windows.profile.borrow_mut() = Some(p.clone());
    p.reload_list();
    let first = p.ids.borrow().first().copied();
    p.select(first);
    p.view.clone()
}

fn with(f: impl FnOnce(&TunnelsPane)) {
    let w = app().windows.profile.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn build() -> TunnelsPane {
    let h = Handlers::new();
    let table = Table::new(&[("Name", 140.0), ("Target", 120.0)]);
    table.set_inset_style();
    table.on_select(|i| {
        with(|p| {
            let id = i.and_then(|i| p.ids.borrow().get(i).copied());
            if id.is_some() {
                p.select(id);
            }
        })
    });
    let add = kit::plain_icon("plus", "New profile", 13.0, &h, || with(|p| p.new_profile()));
    let remove = kit::plain_icon("minus", "Delete profile", 13.0, &h, || {
        with(|p| {
            let e = p.editor.borrow().clone();
            if let Some(e) = e {
                e.delete();
            }
        })
    });
    let import = kit::plain_icon("square.and.arrow.down", "Import ngrok.yml", 13.0, &h, || super::accounts::import_ngrok_yml(None));
    let bar = ui::hstack(&[&add, &remove, &ui::spacer(), &import], 10.0);
    ui::pad(&bar, 4.0, 6.0);
    let left = ui::vstack(&[&table.scroll, &bar], 8.0);
    ui::fill_width(&left, &table.scroll);
    ui::fill_width(&left, &bar);
    table.scroll.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Vertical);

    let editor_host = NSView::new(ui::mtm());
    let root = NSView::new(ui::mtm());
    for v in [&*left as &NSView, &editor_host] {
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.addSubview(v);
    }
    left.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0).setActive(true);
    left.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 20.0).setActive(true);
    left.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -16.0).setActive(true);
    left.widthAnchor().constraintEqualToConstant(LIST_W).setActive(true);
    editor_host.leadingAnchor().constraintEqualToAnchor_constant(&left.trailingAnchor(), 16.0).setActive(true);
    editor_host.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()).setActive(true);
    editor_host.topAnchor().constraintEqualToAnchor(&root.topAnchor()).setActive(true);
    editor_host.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor()).setActive(true);

    TunnelsPane { view: root, _h: h, table, ids: RefCell::new(Vec::new()), editor_host, editor: RefCell::new(None) }
}

impl TunnelsPane {
    /// Refresh the list (names, running state) keeping the selection.
    pub fn reload_list(&self) {
        let app = app();
        let cfg = app.config.borrow().clone();
        let mut profiles = cfg.profiles.clone();
        profiles.sort_by_key(|p| (!p.favorite, p.name.to_lowercase()));
        let rows = profiles
            .iter()
            .map(|p| {
                let running = app.core.running_profile(p.id).is_some();
                let name = format!("{}{}{}", if running { "● " } else { "" }, if p.favorite { "★ " } else { "" }, p.name);
                vec![name, format!("{} → {}", p.protocol().as_str(), p.upstream)]
            })
            .collect();
        let ids: Vec<Uuid> = profiles.iter().map(|p| p.id).collect();
        let current = self.editor.borrow().as_ref().filter(|e| !e.is_new).map(|e| e.base.id);
        let keep = current.and_then(|id| ids.iter().position(|x| *x == id));
        *self.ids.borrow_mut() = ids;
        self.table.set_rows(rows, keep);
    }

    fn select(&self, id: Option<Uuid>) {
        let profile = id.and_then(|id| app().config.borrow().profile(id).cloned());
        match profile {
            Some(p) => self.mount(p, false),
            None => self.mount_empty(),
        }
        self.reload_list();
    }

    fn new_profile(&self) {
        let app = app();
        let cfg = app.config.borrow().clone();
        let Some(acc) = cfg.active_account().map(|a| a.id) else { return };
        let q = app.quick.borrow().clone();
        let mut p = TunnelProfile::new("", acc, q.protocol, q.upstream);
        p.options = ProtocolOptions::for_protocol(q.protocol);
        self.mount(p, true);
        self.reload_list();
    }

    fn set_editor_view(&self, v: &NSView) {
        for old in self.editor_host.subviews().iter() {
            old.removeFromSuperview();
        }
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        self.editor_host.addSubview(v);
        v.leadingAnchor().constraintEqualToAnchor(&self.editor_host.leadingAnchor()).setActive(true);
        v.trailingAnchor().constraintEqualToAnchor(&self.editor_host.trailingAnchor()).setActive(true);
        v.topAnchor().constraintEqualToAnchor(&self.editor_host.topAnchor()).setActive(true);
        v.bottomAnchor().constraintEqualToAnchor(&self.editor_host.bottomAnchor()).setActive(true);
    }

    fn mount(&self, p: TunnelProfile, is_new: bool) {
        let e = Rc::new(Editor::build(p, is_new));
        self.set_editor_view(&e.root);
        *self.editor.borrow_mut() = Some(e);
    }

    fn mount_empty(&self) {
        *self.editor.borrow_mut() = None;
        let icon = kit::symbol_view("arrow.up.and.down.circle", 40.0, Some(&NSColor::tertiaryLabelColor()));
        let t = kit::group_title("No profile selected");
        let n = kit::footnote("Save tunnels you start often as profiles: pick one on the left or press +.", 320.0);
        let s = ui::vstack(&[&icon, &t, &n], 8.0);
        s.setAlignment(objc2_app_kit::NSLayoutAttribute::CenterX);
        let host = NSView::new(ui::mtm());
        s.setTranslatesAutoresizingMaskIntoConstraints(false);
        host.addSubview(&s);
        s.centerXAnchor().constraintEqualToAnchor(&host.centerXAnchor()).setActive(true);
        s.centerYAnchor().constraintEqualToAnchor(&host.centerYAnchor()).setActive(true);
        self.set_editor_view(&host);
    }
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

pub struct Editor {
    root: Retained<NSView>,
    _h: Handlers,
    base: TunnelProfile,
    is_new: bool,
    account_ids: Vec<Uuid>,
    name: Retained<NSTextField>,
    account: Retained<NSPopUpButton>,
    protocol: Retained<NSPopUpButton>,
    upstream: Retained<NSTextField>,
    url: Retained<NSTextField>,
    favorite: Retained<NSSwitch>,
    inspect: Retained<NSSwitch>,
    hotkey: Retained<NSTextField>,
    description: Retained<NSTextField>,
    metadata: Retained<NSTextField>,
    binding: Retained<NSPopUpButton>,
    pooling: Retained<NSSwitch>,
    proxy_proto: Retained<NSPopUpButton>,
    verify_tls: Retained<NSSwitch>,
    allow_cidrs: Retained<NSTextField>,
    deny_cidrs: Retained<NSTextField>,
    policy: Retained<NSTextView>,
    http_sections: Retained<NSStackView>,
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
    host_rewrite: Retained<NSSwitch>,
    compression: Retained<NSSwitch>,
    circuit_breaker: Retained<NSTextField>,
    ws_tcp: Retained<NSSwitch>,
    app_protocol: Retained<NSPopUpButton>,
    http_mtls: Retained<NSTextField>,
    ua_allow: Retained<NSTextField>,
    ua_deny: Retained<NSTextField>,
    tls_sections: Retained<NSStackView>,
    tls_cert: Retained<NSTextField>,
    tls_key: Retained<NSTextField>,
    tls_mtls: Retained<NSTextField>,
    errors: Retained<NSTextField>,
}

fn with_editor(f: impl FnOnce(&Editor)) {
    let e = app().windows.profile.borrow().as_ref().and_then(|p| p.editor.borrow().clone());
    if let Some(e) = e {
        f(&e);
    }
}

fn opt(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// Title above a multi-line editor spanning the row.
fn text_row(title: &str, field: &NSView, height: f64) -> Retained<NSView> {
    let l = ui::label(title);
    ui::set_height(field, height);
    let s = ui::vstack(&[&l, field], 6.0);
    ui::fill_width(&s, field);
    ui::v(&*s)
}

impl Editor {
    fn build(base: TunnelProfile, is_new: bool) -> Editor {
        let h = Handlers::new();
        let cfg = app().config.borrow().clone();
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

        let name = ui::text_field(&base.name, "my-api");
        let account = ui::popup(&account_names, account_ids.iter().position(|i| *i == base.account_id).unwrap_or(0));
        let protocol = ui::popup(&protos, Protocol::ALL.iter().position(|p| *p == base.protocol()).unwrap_or(0));
        let upstream = ui::text_field(&base.upstream, "3000, localhost:8080, https://local.test:8443");
        let url = ui::text_field(&base.url, "");
        let favorite = kit::switch(base.favorite);
        let inspect = kit::switch(base.inspect);
        let hotkey = ui::text_field(&hotkey_value, "e.g. cmd+shift+1");

        let description = ui::text_field(&base.description, "Shown as “forwards to” in the dashboard");
        let metadata = ui::text_field(&base.metadata, "Optional");
        let binding = ui::popup(&Binding::ALL.map(|b| b.as_str()), Binding::ALL.iter().position(|b| *b == base.binding).unwrap_or(0));
        let pooling = kit::switch(base.pooling);
        let proxy_proto = ui::popup(&PROXY_PROTO, base.proxy_proto as usize);
        let verify_tls = kit::switch(base.verify_upstream_tls);
        let allow_cidrs = ui::text_field(&join_list(&base.allow_cidrs), "10.0.0.0/8, 203.0.113.4/32");
        let deny_cidrs = ui::text_field(&join_list(&base.deny_cidrs), "");
        let (policy_scroll, policy) = ui::text_area(&base.traffic_policy, true, true);

        let scheme = ui::popup(&SCHEMES, if http.scheme.as_deref() == Some("http") { 1 } else { 0 });
        let ba_text: Vec<String> = http.basic_auth.iter().map(|b| format!("{}:{}", b.username, b.password.expose())).collect();
        let (ba_scroll, basic_auth) = ui::text_area(&ba_text.join("\n"), true, true);
        let oauth_provider = ui::popup(&OAUTH_PROVIDERS, OAUTH_PROVIDERS.iter().position(|p| *p == oauth.provider).unwrap_or(0));
        let oauth_emails = ui::text_field(&join_list(&oauth.allow_emails), "Allowed emails");
        let oauth_domains = ui::text_field(&join_list(&oauth.allow_domains), "Allowed domains");
        let oauth_scopes = ui::text_field(&join_list(&oauth.scopes), "Extra scopes");
        let oauth_client_id = ui::text_field(&oauth.client_id, "Custom OAuth app (optional)");
        let oauth_client_secret = ui::secure_field(oauth.client_secret.expose(), "Client secret");
        let oidc_issuer = ui::text_field(&oidc.issuer_url, "https://accounts.example.com");
        let oidc_client_id = ui::text_field(&oidc.client_id, "Client ID");
        let oidc_client_secret = ui::secure_field(oidc.client_secret.expose(), "Client secret");
        let oidc_emails = ui::text_field(&join_list(&oidc.allow_emails), "Allowed emails");
        let oidc_domains = ui::text_field(&join_list(&oidc.allow_domains), "Allowed domains");
        let oidc_scopes = ui::text_field(&join_list(&oidc.scopes), "Scopes");
        let webhook_provider = ui::text_field(&webhook.provider, "github, stripe, slack, twilio…");
        let webhook_secret = ui::secure_field(webhook.secret.expose(), "Signing secret");
        let req_add_text: Vec<String> = http.request_headers_add.iter().map(|h| h.to_string()).collect();
        let (req_add_scroll, req_add) = ui::text_area(&req_add_text.join("\n"), true, true);
        let req_remove = ui::text_field(&join_list(&http.request_headers_remove), "Cookie, X-Debug");
        let resp_add_text: Vec<String> = http.response_headers_add.iter().map(|h| h.to_string()).collect();
        let (resp_add_scroll, resp_add) = ui::text_area(&resp_add_text.join("\n"), true, true);
        let resp_remove = ui::text_field(&join_list(&http.response_headers_remove), "Server");
        let host_rewrite = kit::switch(http.host_header_rewrite);
        let compression = kit::switch(http.compression);
        let circuit_breaker = ui::text_field(&http.circuit_breaker.map(|c| c.to_string()).unwrap_or_default(), "Off · 0.5 = 50% errors");
        let ws_tcp = kit::switch(http.websocket_tcp_conversion);
        let app_protocol = ui::popup(&APP_PROTO, if http.app_protocol.as_deref() == Some("http2") { 1 } else { 0 });
        let http_mtls = ui::text_field(http.mutual_tls_ca_path.as_deref().unwrap_or(""), "CA bundle (PEM)");
        let ua_allow = ui::text_field(&join_list(&http.allow_user_agents), "Regexes");
        let ua_deny = ui::text_field(&join_list(&http.deny_user_agents), "Regexes");

        let tls_cert = ui::text_field(tls.cert_path.as_deref().unwrap_or(""), "Certificate (PEM) — terminate at edge");
        let tls_key = ui::text_field(tls.key_path.as_deref().unwrap_or(""), "Private key (PEM)");
        let tls_mtls = ui::text_field(tls.mutual_tls_ca_path.as_deref().unwrap_or(""), "CA bundle (PEM)");

        let browse = |field: &Retained<NSTextField>| -> Retained<NSView> {
            let f = field.clone();
            let b = kit::glass_button("Choose…", None, false, &h, move || {
                if let Some(p) = ui::open_panel(None) {
                    f.setStringValue(&ns(&p.to_string_lossy()));
                }
            });
            b.setControlSize(objc2_app_kit::NSControlSize::Small);
            ui::v(&*ui::fill_hstack(&[&**field, &b], 6.0))
        };

        // Header.
        let tint = match base.protocol() {
            Protocol::Http => NSColor::systemIndigoColor(),
            Protocol::Tcp => NSColor::systemTealColor(),
            Protocol::Tls => NSColor::systemPurpleColor(),
        };
        let tile = kit::icon_tile("arrow.up.and.down", &tint, 40.0);
        let title = ui::label(if is_new { "New Profile" } else { &base.name });
        title.setFont(Some(&NSFont::systemFontOfSize_weight(20.0, weight::BOLD)));
        let subtitle = ui::secondary_label(if is_new {
            "Configure the endpoint, then save."
        } else {
            "Changes apply the next time the tunnel starts."
        });
        let head_texts = ui::vstack(&[&title, &subtitle], 2.0);
        let header = ui::hstack(&[&tile, &head_texts], 14.0);

        let general = kit::section(
            Some("General"),
            &[
                kit::row_fill("Name", &name, LW),
                kit::row("Account", &account),
                kit::row("Protocol", &protocol),
                kit::row_fill("Local service", &upstream, LW),
                kit::row_fill("Public URL", &url, LW),
                kit::row("Favorite", &favorite),
                kit::row_detail("Inspect traffic", "Capture HTTP requests in the Inspector", &inspect),
                kit::row_fill("Keyboard shortcut", &hotkey, LW),
            ],
            None,
        );
        let endpoint = kit::section(
            Some("Endpoint"),
            &[
                kit::row_fill("Description", &description, LW),
                kit::row_fill("Metadata", &metadata, LW),
                kit::row("Binding", &binding),
                kit::row_detail("Pooling", "Load-balance with other agents on the same URL", &pooling),
                kit::row("PROXY protocol", &proxy_proto),
                kit::row("Verify upstream TLS", &verify_tls),
                kit::row_fill("Allow IPs (CIDR)", &allow_cidrs, LW),
                kit::row_fill("Deny IPs (CIDR)", &deny_cidrs, LW),
            ],
            None,
        );
        let access = kit::section(
            Some("Access Control"),
            &[
                kit::row("Scheme", &scheme),
                text_row("Basic auth — one user:password per line", &ba_scroll, 52.0),
                kit::row("OAuth provider", &oauth_provider),
                kit::row_fill("OAuth emails", &oauth_emails, LW),
                kit::row_fill("OAuth domains", &oauth_domains, LW),
                kit::row_fill("OAuth scopes", &oauth_scopes, LW),
                kit::row_fill("OAuth client ID", &oauth_client_id, LW),
                kit::row_fill("OAuth client secret", &oauth_client_secret, LW),
                kit::row_fill("OIDC issuer", &oidc_issuer, LW),
                kit::row_fill("OIDC client ID", &oidc_client_id, LW),
                kit::row_fill("OIDC client secret", &oidc_client_secret, LW),
                kit::row_fill("OIDC emails", &oidc_emails, LW),
                kit::row_fill("OIDC domains", &oidc_domains, LW),
                kit::row_fill("OIDC scopes", &oidc_scopes, LW),
                kit::row_fill("Webhook provider", &webhook_provider, LW),
                kit::row_fill("Webhook secret", &webhook_secret, LW),
                kit::row_fill("Allow user agents", &ua_allow, LW),
                kit::row_fill("Deny user agents", &ua_deny, LW),
            ],
            Some("Secrets are stored in your Keychain."),
        );
        let headers = kit::section(
            Some("Headers"),
            &[
                text_row("Add to requests — Name: value per line", &req_add_scroll, 52.0),
                kit::row_fill("Remove from requests", &req_remove, LW),
                text_row("Add to responses — Name: value per line", &resp_add_scroll, 52.0),
                kit::row_fill("Remove from responses", &resp_remove, LW),
                kit::row_detail("Rewrite Host header", "Send the local service's host instead of the public one", &host_rewrite),
            ],
            None,
        );
        let advanced = kit::section(
            Some("Advanced"),
            &[
                kit::row("Compress responses", &compression),
                kit::row_fill("Circuit breaker", &circuit_breaker, LW),
                kit::row("WebSocket → TCP", &ws_tcp),
                kit::row("Upstream protocol", &app_protocol),
                kit::row_fill("Mutual TLS CAs", &browse(&http_mtls), LW),
            ],
            None,
        );
        let http_sections = ui::vstack(&[&access, &headers, &advanced], 28.0);
        for v in [&*access as &NSView, &headers, &advanced] {
            ui::fill_width(&http_sections, v);
        }
        let tls_section = kit::section(
            Some("TLS"),
            &[
                kit::row_fill("Certificate", &browse(&tls_cert), LW),
                kit::row_fill("Private key", &browse(&tls_key), LW),
                kit::row_fill("Mutual TLS CAs", &browse(&tls_mtls), LW),
            ],
            Some("Leave certificate and key empty to pass TLS through to your service."),
        );
        let tls_sections = ui::vstack(&[&tls_section], 0.0);
        ui::fill_width(&tls_sections, &tls_section);

        let policy_section = kit::section(
            Some("Traffic Policy"),
            &[text_row("YAML or JSON", &policy_scroll, 150.0)],
            Some(
                "Rules ngrok applies to this endpoint: rate limits, redirects, auth, custom responses… See ngrok.com/docs/traffic-policy.",
            ),
        );

        let errors = ui::wrapping_label("", 560.0);
        errors.setTextColor(Some(&NSColor::systemRedColor()));

        let content = ui::vstack(&[&header, &general, &endpoint, &http_sections, &tls_sections, &policy_section, &errors], 28.0);
        for v in [&*general as &NSView, &endpoint, &http_sections, &tls_sections, &policy_section, &errors] {
            ui::fill_width(&content, v);
        }
        ui::pad_edges(&content, 20.0, 24.0, 28.0, 24.0);
        let scroll = ui::scroll_stack(&content);

        // Bottom bar.
        let delete = kit::glass_button("Delete…", Some("trash"), false, &h, || with_editor(|e| e.delete()));
        delete.setHidden(is_new);
        let export = kit::glass_button("Export…", Some("square.and.arrow.up"), false, &h, || with_editor(|e| e.export()));
        let save = kit::glass_button("Save", None, false, &h, || with_editor(|e| e.save(false)));
        save.setKeyEquivalent(&ns("s"));
        save.setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags::Command);
        let save_start = kit::glass_button("Save & Start", Some("play.fill"), true, &h, || with_editor(|e| e.save(true)));
        let bar = ui::hstack(&[&delete, &export, &ui::spacer(), &save, &save_start], 8.0);
        ui::pad(&bar, 12.0, 24.0);

        let sep = ui::separator();
        let root = ui::vstack(&[&scroll, &sep, &bar], 0.0);
        for v in [&*scroll as &NSView, &sep, &bar] {
            ui::fill_width(&root, v);
        }
        scroll.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Vertical);

        let e = Editor {
            root: ui::v(&*root),
            _h: h,
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
            http_sections,
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
            tls_sections,
            tls_cert,
            tls_key,
            tls_mtls,
            errors,
        };
        ui::on_action(&e.protocol, &e._h, || with_editor(|e| e.protocol_changed()));
        e.protocol_changed();
        e
    }

    fn current_protocol(&self) -> Protocol {
        Protocol::ALL[ui::selected(&self.protocol).min(2)]
    }

    fn protocol_changed(&self) {
        let p = self.current_protocol();
        self.http_sections.setHidden(p != Protocol::Http);
        self.tls_sections.setHidden(p != Protocol::Tls);
        self.inspect.setEnabled(p == Protocol::Http);
        let placeholder = match p {
            Protocol::Http => "my-app.ngrok.app — empty for a random URL",
            Protocol::Tcp => "1.tcp.ngrok.io:12345 — empty for a random address",
            Protocol::Tls => "secure.example.com",
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
        p.favorite = kit::is_on(&self.favorite);
        p.inspect = kit::is_on(&self.inspect);
        p.description = ui::value(&self.description);
        p.metadata = ui::value(&self.metadata);
        p.binding = Binding::ALL[ui::selected(&self.binding).min(2)];
        p.pooling = kit::is_on(&self.pooling);
        p.proxy_proto = [ProxyProtoVersion::None, ProxyProtoVersion::V1, ProxyProtoVersion::V2][ui::selected(&self.proxy_proto).min(2)];
        p.verify_upstream_tls = kit::is_on(&self.verify_tls);
        p.allow_cidrs = split_list(&ui::value(&self.allow_cidrs));
        p.deny_cidrs = split_list(&ui::value(&self.deny_cidrs));
        p.traffic_policy = ui::text_of(&self.policy);

        p.options = match self.current_protocol() {
            Protocol::Http => {
                let mut h = HttpOptions { scheme: (ui::selected(&self.scheme) == 1).then(|| "http".to_string()), ..Default::default() };
                for line in ui::text_of(&self.basic_auth).lines().map(str::trim).filter(|l| !l.is_empty()) {
                    match line.split_once(':') {
                        Some((u, pw)) => h.basic_auth.push(BasicAuth { username: u.trim().into(), password: pw.into() }),
                        None => errs.push(format!("basic auth line `{line}` must be user:password")),
                    }
                }
                let provider = OAUTH_PROVIDERS[ui::selected(&self.oauth_provider).min(OAUTH_PROVIDERS.len() - 1)];
                if provider != "None" {
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
                h.host_header_rewrite = kit::is_on(&self.host_rewrite);
                h.compression = kit::is_on(&self.compression);
                let cb = ui::value(&self.circuit_breaker);
                if !cb.is_empty() {
                    match cb.parse::<f64>() {
                        Ok(v) => h.circuit_breaker = Some(v),
                        Err(_) => errs.push("circuit breaker must be a number between 0 and 1".into()),
                    }
                }
                h.websocket_tcp_conversion = kit::is_on(&self.ws_tcp);
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

    fn show_errors(&self, errs: &[String]) {
        self.errors.setStringValue(&ns(&format!("Please fix:\n• {}", errs.join("\n• "))));
    }

    fn save(&self, start: bool) {
        let p = match self.collect() {
            Ok(p) => p,
            Err(errs) => return self.show_errors(&errs),
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
        let id = p.id;
        if start || running {
            if running {
                app.core.stop_profile(id);
                crate::platform::on_main(move || restart_when_stopped(id, 0));
            } else {
                app.start_profile(id);
            }
        }
        // Remount so the editor reflects the saved state (title, delete button).
        crate::platform::on_main(move || with(|pane| pane.select(Some(id))));
    }

    fn delete(&self) {
        if self.is_new {
            with(|pane| pane.select(None));
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
        crate::platform::on_main(|| {
            with(|pane| {
                pane.reload_list();
                let first = pane.ids.borrow().first().copied();
                pane.select(first);
            })
        });
    }

    fn export(&self) {
        let p = match self.collect() {
            Ok(p) => p,
            Err(errs) => return self.show_errors(&errs),
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
