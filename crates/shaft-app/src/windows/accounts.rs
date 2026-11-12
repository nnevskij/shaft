//! Accounts pane: authtokens and API keys (Keychain), connection settings,
//! ngrok.yml import/export (`ngrok config …` parity).

use std::{cell::Cell, rc::Rc};

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSColor, NSFont, NSSecureTextField, NSTextField, NSView};
use shaft_config::{default_ngrok_yml, ngrok_yml, Account, TunnelProfile};
use uuid::Uuid;

use super::main::Section;
use crate::{
    app::app,
    kit::{self, weight},
    platform::on_main,
    ui::{self, ns, Handlers, Table},
};

const LW: f64 = 130.0;

pub struct AccountsPane {
    pub view: Retained<NSView>,
    _h: Handlers,
    table: Table,
    selected: Cell<Option<Uuid>>,
    title: Retained<NSTextField>,
    name: Retained<NSTextField>,
    authtoken: Retained<NSSecureTextField>,
    api_key: Retained<NSSecureTextField>,
    server_addr: Retained<NSTextField>,
    proxy_url: Retained<NSTextField>,
    root_cas: Retained<NSTextField>,
    status: Retained<NSTextField>,
    remove: Retained<NSButton>,
    detail: Retained<NSView>,
}

pub fn open() {
    super::main::show(Section::Accounts);
}

pub fn pane() -> Retained<NSView> {
    let p = Rc::new(build());
    *app().windows.accounts.borrow_mut() = Some(p.clone());
    p.reload_list();
    let first = app().config.borrow().active_account().map(|a| a.id);
    p.select(first);
    p.view.clone()
}

fn with(f: impl FnOnce(&AccountsPane)) {
    let w = app().windows.accounts.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn build() -> AccountsPane {
    let h = Handlers::new();
    let table = Table::new(&[("Account", 150.0), ("Status", 90.0)]);
    table.set_inset_style();
    table.on_select(|i| {
        with(|w| {
            let id = i.and_then(|i| app().config.borrow().accounts.get(i).map(|a| a.id));
            if id.is_some() {
                w.select(id);
            }
        })
    });
    let add = kit::plain_icon("plus", "Add account", 13.0, &h, || {
        let app = app();
        let n = app.config.borrow().accounts.len() + 1;
        let a = Account::new(format!("Account {n}"));
        let id = a.id;
        app.config.borrow_mut().upsert_account(a);
        app.save();
        with(|w| {
            w.reload_list();
            w.select(Some(id));
        });
    });
    let remove = kit::plain_icon("minus", "Remove account", 13.0, &h, || with(|w| w.remove()));
    let import = kit::plain_icon("square.and.arrow.down", "Import ngrok.yml", 13.0, &h, || {
        let target = app().windows.accounts.borrow().as_ref().and_then(|w| w.selected.get());
        import_ngrok_yml(target);
    });
    let export = kit::plain_icon("square.and.arrow.up", "Export ngrok.yml", 13.0, &h, || {
        with(|w| {
            let Some(id) = w.selected.get() else { return };
            let cfg = app().config.borrow().clone();
            let Some(acc) = cfg.account(id) else { return };
            let profiles: Vec<TunnelProfile> = cfg.profiles.iter().filter(|p| p.account_id == id).cloned().collect();
            export_profiles(acc, &profiles);
        })
    });
    let bar = ui::hstack(&[&add, &remove, &ui::spacer(), &import, &export], 10.0);
    ui::pad(&bar, 4.0, 6.0);
    let left = ui::vstack(&[&table.scroll, &bar], 8.0);
    ui::fill_width(&left, &table.scroll);
    ui::fill_width(&left, &bar);
    table.scroll.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Vertical);

    // Detail.
    let tile = kit::icon_tile("person.fill", &NSColor::systemGreenColor(), 40.0);
    let title = ui::label("");
    title.setFont(Some(&NSFont::systemFontOfSize_weight(20.0, weight::BOLD)));
    let use_btn = kit::glass_button("Use for Quick Tunnels", Some("checkmark.circle"), false, &h, || {
        with(|w| {
            if let Some(id) = w.selected.get() {
                let app = app();
                app.config.borrow_mut().settings.active_account = Some(id);
                app.save();
                w.reload_list();
                w.status.setStringValue(&ns("This account is now used for quick tunnels and resources."));
            }
        })
    });
    let header = ui::fill_hstack(&[&tile, &title, &ui::spacer(), &use_btn], 12.0);

    let name = ui::text_field("", "Personal, Work…");
    let authtoken = ui::secure_field("", "From dashboard.ngrok.com");
    let api_key = ui::secure_field("", "Optional");
    let server_addr = ui::text_field("", "connect.ngrok-agent.com:443");
    let proxy_url = ui::text_field("", "http://proxy:3128 or socks5://…");
    let root_cas = ui::text_field("", "PEM path, “trusted” or “host”");
    let get_token = ui::link_button("Get authtoken ↗", &h, || ui::open_url("https://dashboard.ngrok.com/get-started/your-authtoken"));
    let get_key = ui::link_button("Create API key ↗", &h, || ui::open_url("https://dashboard.ngrok.com/api-keys"));

    let account_section = kit::section(Some("Account"), &[kit::row_fill("Name", &name, LW)], None);
    let creds = kit::section(
        Some("Credentials"),
        &[kit::row_fill("Authtoken", &authtoken, LW), kit::row_fill("API key", &api_key, LW)],
        Some("The authtoken opens tunnels; the API key manages resources (domains, endpoints…). Both are stored in your Keychain."),
    );
    let links = ui::hstack(&[&get_token, &get_key], 20.0);
    ui::pad_edges(&links, 0.0, kit::space::CARD_H - 4.0, 0.0, 0.0);
    let conn = kit::section(
        Some("Connection"),
        &[
            kit::row_fill("Server address", &server_addr, LW),
            kit::row_fill("Proxy", &proxy_url, LW),
            kit::row_fill("Root CAs", &root_cas, LW),
        ],
        Some("Leave empty for defaults. Changes apply to sessions started afterwards."),
    );
    let status = kit::footnote("", 560.0);
    let validate = kit::glass_button("Validate", Some("checkmark.shield"), false, &h, || with(|w| w.validate()));
    let save = kit::glass_button("Save", None, true, &h, || with(|w| w.save()));
    save.setKeyEquivalent(&ns("s"));
    save.setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags::Command);
    let buttons = ui::hstack(&[&validate, &ui::spacer(), &save], 8.0);

    let detail_stack = ui::vstack(&[&header, &account_section, &creds, &links, &conn, &buttons, &status], 28.0);
    for v in [&*header as &NSView, &account_section, &creds, &conn, &buttons, &status] {
        ui::fill_width(&detail_stack, v);
    }
    ui::pad_edges(&detail_stack, 20.0, 24.0, 28.0, 24.0);
    let detail_scroll = ui::scroll_stack(&detail_stack);

    let root = NSView::new(ui::mtm());
    for v in [&*left as &NSView, &detail_scroll] {
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.addSubview(v);
    }
    left.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0).setActive(true);
    left.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 20.0).setActive(true);
    left.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -16.0).setActive(true);
    left.widthAnchor().constraintEqualToConstant(260.0).setActive(true);
    detail_scroll.leadingAnchor().constraintEqualToAnchor_constant(&left.trailingAnchor(), 16.0).setActive(true);
    detail_scroll.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()).setActive(true);
    detail_scroll.topAnchor().constraintEqualToAnchor(&root.topAnchor()).setActive(true);
    detail_scroll.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor()).setActive(true);

    let remove_btn = remove.clone();
    AccountsPane {
        view: root,
        _h: h,
        table,
        selected: Cell::new(None),
        title,
        name,
        authtoken,
        api_key,
        server_addr,
        proxy_url,
        root_cas,
        status,
        remove: remove_btn,
        detail: ui::v(&*detail_scroll),
    }
}

impl AccountsPane {
    pub fn reload_list(&self) {
        let cfg = app().config.borrow().clone();
        let active = cfg.active_account().map(|a| a.id);
        let rows = cfg
            .accounts
            .iter()
            .map(|a| {
                let name = if Some(a.id) == active { format!("{} ✓", a.name) } else { a.name.clone() };
                let status = match (a.authtoken.is_empty(), a.api_key.is_empty()) {
                    (true, _) => "No authtoken",
                    (false, true) => "Tunnels",
                    (false, false) => "Tunnels + API",
                };
                vec![name, status.to_string()]
            })
            .collect();
        let keep = self.selected.get().and_then(|id| cfg.accounts.iter().position(|a| a.id == id));
        self.table.set_rows(rows, keep);
    }

    fn select(&self, id: Option<Uuid>) {
        self.selected.set(id);
        let cfg = app().config.borrow().clone();
        let a = id.and_then(|id| cfg.account(id).cloned());
        self.detail.setHidden(a.is_none());
        self.remove.setEnabled(a.is_some());
        let a = a.unwrap_or_else(|| Account::new(""));
        self.title.setStringValue(&ns(&a.name));
        self.name.setStringValue(&ns(&a.name));
        self.authtoken.setStringValue(&ns(a.authtoken.expose()));
        self.api_key.setStringValue(&ns(a.api_key.expose()));
        self.server_addr.setStringValue(&ns(a.server_addr.as_deref().unwrap_or("")));
        self.proxy_url.setStringValue(&ns(a.proxy_url.as_deref().unwrap_or("")));
        self.root_cas.setStringValue(&ns(a.root_cas.as_deref().unwrap_or("")));
        self.status.setStringValue(&ns(""));
        self.reload_list();
    }

    fn collect(&self) -> Option<Account> {
        let id = self.selected.get()?;
        let mut a = app().config.borrow().account(id).cloned()?;
        a.name = ui::value(&self.name);
        a.authtoken = self.authtoken.stringValue().to_string().trim().into();
        a.api_key = self.api_key.stringValue().to_string().trim().into();
        let opt = |s: String| (!s.is_empty()).then_some(s);
        a.server_addr = opt(ui::value(&self.server_addr));
        a.proxy_url = opt(ui::value(&self.proxy_url));
        a.root_cas = opt(ui::value(&self.root_cas));
        Some(a)
    }

    fn save(&self) {
        let Some(a) = self.collect() else { return };
        if a.name.is_empty() {
            self.status.setStringValue(&ns("Name is required."));
            return;
        }
        let app = app();
        self.title.setStringValue(&ns(&a.name));
        app.config.borrow_mut().upsert_account(a);
        if app.save() {
            self.status.setStringValue(&ns("Saved. Running tunnels keep their connection until restarted."));
            self.reload_list();
        }
    }

    fn validate(&self) {
        let Some(a) = self.collect() else { return };
        self.status.setStringValue(&ns("Checking…"));
        let status = SendPtr(self.status.clone());
        app().core.runtime().spawn(async move {
            let (t, k) = tokio::join!(shaft_core::diagnostics::check_authtoken(&a), shaft_core::diagnostics::check_api_key(&a));
            let text = format!("Authtoken: {}\nAPI key: {}", t.detail, k.detail);
            on_main(move || status.get().setStringValue(&ns(&text)));
        });
    }

    fn remove(&self) {
        let Some(id) = self.selected.get() else { return };
        let app = app();
        let (name, n_profiles) = {
            let cfg = app.config.borrow();
            (cfg.account(id).map(|a| a.name.clone()).unwrap_or_default(), cfg.profiles.iter().filter(|p| p.account_id == id).count())
        };
        let info = format!(
            "Its tunnels will be stopped, {n_profiles} profile(s) deleted and its authtoken/API key removed from the Keychain. This cannot be undone."
        );
        if !ui::confirm(&format!("Remove account “{name}”?"), &info, "Remove", true) {
            return;
        }
        app.core.stop_account(id);
        let removed = app.config.borrow_mut().remove_account(id);
        for p in &removed {
            let _ = app.store.forget_profile_secrets(p);
        }
        if let Err(e) = app.store.forget_account_secrets(id) {
            tracing::warn!(error = %e, "could not remove account secrets");
        }
        app.save();
        let next = app.config.borrow().accounts.first().map(|a| a.id);
        self.select(next);
    }
}

/// `Retained` UI objects are not `Send`; this wrapper is only ever
/// dereferenced back on the main thread (inside `on_main`).
pub struct SendPtr<T>(pub T);
unsafe impl<T> Send for SendPtr<T> {}

impl<T> SendPtr<T> {
    /// Access the value (a method call captures the whole wrapper in closures).
    pub fn get(&self) -> &T {
        &self.0
    }
}

/// Validate an authtoken by opening a session, then add the account
/// (used by onboarding).
pub fn validate_and_add(account: Account, status: Retained<NSTextField>) {
    let rt = app().core.runtime().clone();
    let status = SendPtr(status);
    rt.spawn(async move {
        let check = shaft_core::diagnostics::check_authtoken(&account).await;
        on_main(move || {
            let app = app();
            if check.status == shaft_core::diagnostics::CheckStatus::Ok {
                let a = account;
                let id = a.id;
                app.config.borrow_mut().upsert_account(a);
                app.config.borrow_mut().settings.active_account = Some(id);
                if app.save() {
                    crate::app::show_popover();
                }
            } else {
                status.get().setStringValue(&ns(&format!("Authtoken rejected: {}", check.detail)));
            }
        });
    });
}

/// Import an ngrok.yml into `target` (or a new account).
pub fn import_ngrok_yml(target: Option<Uuid>) {
    let app = app();
    app.close_popover();
    let default = default_ngrok_yml();
    let path = if default.exists()
        && ui::confirm("Import ngrok.yml", &format!("Import the ngrok agent config found at\n{}?", default.display()), "Import", false)
    {
        Some(default)
    } else {
        ui::open_panel(default.parent())
    };
    let Some(path) = path else { return };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return ui::error("Cannot read file", &format!("{}: {e}", path.display())),
    };
    let account_id = target.unwrap_or_else(Uuid::new_v4);
    let res = match ngrok_yml::import(&text, account_id) {
        Ok(r) => r,
        Err(e) => return ui::error("Invalid ngrok.yml", &e.to_string()),
    };

    let mut cfg = app.config.borrow().clone();
    let mut account = cfg.account(account_id).cloned().unwrap_or_else(|| {
        let mut a = Account::new(path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into()));
        a.id = account_id;
        a
    });
    if let Some(t) = res.authtoken.filter(|t| !t.is_empty()) {
        account.authtoken = t.into();
    }
    if let Some(k) = res.api_key.filter(|k| !k.is_empty()) {
        account.api_key = k.into();
    }
    if res.server_addr.is_some() {
        account.server_addr = res.server_addr;
    }
    if res.proxy_url.is_some() {
        account.proxy_url = res.proxy_url;
    }
    if res.root_cas.is_some() {
        account.root_cas = res.root_cas;
    }
    let mut imported = 0;
    let mut renamed = Vec::new();
    for mut p in res.profiles {
        let base = p.name.clone();
        let mut n = 2;
        while cfg.profiles.iter().any(|o| o.name.eq_ignore_ascii_case(&p.name)) {
            p.name = format!("{base} ({n})");
            n += 1;
        }
        if p.name != base {
            renamed.push(format!("`{base}` → `{}`", p.name));
        }
        cfg.profiles.push(p);
        imported += 1;
    }
    cfg.upsert_account(account);
    if cfg.settings.active_account.is_none() {
        cfg.settings.active_account = Some(account_id);
    }
    *app.config.borrow_mut() = cfg;
    if !app.save() {
        return;
    }
    let mut msg = format!("Imported {imported} profile(s) from {}.", path.display());
    if !renamed.is_empty() {
        msg.push_str(&format!("\n\nRenamed to avoid duplicates: {}", renamed.join(", ")));
    }
    if !res.warnings.is_empty() {
        msg.push_str(&format!("\n\nWarnings:\n• {}", res.warnings.join("\n• ")));
    }
    msg.push_str("\n\nThe original file was not modified.");
    ui::info("Import complete", &msg);
    let w = app.windows.accounts.borrow().clone();
    if let Some(w) = w {
        w.reload_list();
    }
}

/// Write profiles as a v3 ngrok.yml chosen by the user.
pub fn export_profiles(account: &Account, profiles: &[TunnelProfile]) {
    let include = !account.authtoken.is_empty()
        && ui::confirm(
            "Include secrets?",
            "Include the authtoken, API key and passwords/client secrets in the exported file? Choose Cancel to export placeholders instead.",
            "Include secrets",
            false,
        );
    let res = ngrok_yml::export(account, profiles, include);
    let Some(path) = ui::save_panel("ngrok.yml") else { return };
    if let Err(e) = std::fs::write(&path, &res.yaml) {
        return ui::error("Export failed", &e.to_string());
    }
    #[cfg(unix)]
    if include {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let mut msg = format!("Wrote {} profile(s) to {}.", profiles.len(), path.display());
    if !res.warnings.is_empty() {
        msg.push_str(&format!("\n\nWarnings:\n• {}", res.warnings.join("\n• ")));
    }
    ui::info("Export complete", &msg);
}
