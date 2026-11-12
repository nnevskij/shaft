//! Accounts: authtokens and API keys (Keychain), connection settings,
//! ngrok.yml import/export (`ngrok config …` parity).

use std::{cell::Cell, rc::Rc};

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSSecureTextField, NSTextField, NSView, NSWindow};
use shaft_config::{default_ngrok_yml, ngrok_yml, Account, TunnelProfile};
use uuid::Uuid;

use crate::{
    app::app,
    platform::on_main,
    ui::{self, ns, Handlers, Table},
};

pub struct AccountsWindow {
    pub window: Retained<NSWindow>,
    _handlers: Handlers,
    table: Table,
    selected: Cell<Option<Uuid>>,
    name: Retained<NSTextField>,
    authtoken: Retained<NSSecureTextField>,
    api_key: Retained<NSSecureTextField>,
    server_addr: Retained<NSTextField>,
    proxy_url: Retained<NSTextField>,
    root_cas: Retained<NSTextField>,
    status: Retained<NSTextField>,
    remove: Retained<NSButton>,
}

pub fn open() {
    let app = app();
    app.close_popover();
    let existing = app.windows.accounts.borrow().clone();
    if let Some(w) = existing {
        w.reload_list();
        ui::show_window(&w.window);
        return;
    }
    let w = Rc::new(build());
    *app.windows.accounts.borrow_mut() = Some(w.clone());
    w.reload_list();
    let first = app.config.borrow().active_account().map(|a| a.id);
    w.select(first);
    ui::show_window(&w.window);
}

fn with(f: impl FnOnce(&AccountsWindow)) {
    let w = app().windows.accounts.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn build() -> AccountsWindow {
    let h = Handlers::new();
    let window = ui::window("Accounts", 860.0, 460.0, true);
    let table = Table::new(&[("Account", 160.0), ("Authtoken", 80.0), ("API key", 70.0)]);
    ui::set_width(&table.scroll, 400.0);
    table.on_select(|i| {
        with(|w| {
            let id = i.and_then(|i| app().config.borrow().accounts.get(i).map(|a| a.id));
            w.select(id);
        })
    });

    let name = ui::text_field("", "Personal, Work…");
    let authtoken = ui::secure_field("", "from dashboard.ngrok.com");
    let api_key = ui::secure_field("", "optional (for resources)");
    let server_addr = ui::text_field("", "connect.ngrok-agent.com:443 (default)");
    let proxy_url = ui::text_field("", "http://proxy:3128 or socks5://… (optional)");
    let root_cas = ui::text_field("", "PEM path, `trusted` or `host` (optional)");
    let status = ui::wrapping_label("", 340.0);
    let form = ui::form(
        &[
            ("Name", &name),
            ("Authtoken", &authtoken),
            ("API key", &api_key),
            ("Server address", &server_addr),
            ("Proxy URL", &proxy_url),
            ("Root CAs", &root_cas),
        ],
        110.0,
    );
    let save = ui::button("Save", &h, || with(|w| w.save()));
    save.setKeyEquivalent(&ns("\r"));
    let validate = ui::button("Validate", &h, || with(|w| w.validate()));
    let make_active = ui::button("Use for quick tunnels", &h, || {
        with(|w| {
            if let Some(id) = w.selected.get() {
                let app = app();
                app.config.borrow_mut().settings.active_account = Some(id);
                app.save();
                w.status.setStringValue(&ns("This account is now used for quick tunnels and resources."));
            }
        })
    });
    let tokens =
        ui::link_button("Get authtoken / API key ↗", &h, || ui::open_url("https://dashboard.ngrok.com/get-started/your-authtoken"));
    let detail_buttons = ui::hstack(&[&validate, &make_active, &ui::spacer(), &save], 8.0);
    let detail = ui::vstack(&[&form, &detail_buttons, &status, &tokens], 12.0);
    for v in [&*form as &NSView, &detail_buttons] {
        ui::fill_width(&detail, v);
    }

    let add = ui::button("Add", &h, || {
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
    let remove = ui::button("Remove…", &h, || with(|w| w.remove()));
    let import = ui::button("Import…", &h, || {
        let target = app().windows.accounts.borrow().as_ref().and_then(|w| w.selected.get());
        import_ngrok_yml(target);
    });
    let export = ui::button("Export…", &h, || {
        with(|w| {
            let Some(id) = w.selected.get() else { return };
            let app = app();
            let cfg = app.config.borrow().clone();
            let Some(acc) = cfg.account(id) else { return };
            let profiles: Vec<TunnelProfile> = cfg.profiles.iter().filter(|p| p.account_id == id).cloned().collect();
            export_profiles(acc, &profiles);
        })
    });
    let list_buttons = ui::hstack(&[&add, &remove, &ui::spacer(), &import, &export], 8.0);
    let left = ui::vstack(&[&table.scroll, &list_buttons], 8.0);
    ui::fill_width(&left, &table.scroll);
    ui::fill_width(&left, &list_buttons);
    ui::set_min_height(&table.scroll, 300.0);

    let root = ui::hstack(&[&left, &detail], 16.0);
    root.setAlignment(objc2_app_kit::NSLayoutAttribute::Top);
    ui::set_window_content(&window, &root);

    AccountsWindow {
        window,
        _handlers: h,
        table,
        selected: Cell::new(None),
        name,
        authtoken,
        api_key,
        server_addr,
        proxy_url,
        root_cas,
        status,
        remove,
    }
}

impl AccountsWindow {
    pub fn reload_list(&self) {
        let cfg = app().config.borrow().clone();
        let active = cfg.active_account().map(|a| a.id);
        let rows = cfg
            .accounts
            .iter()
            .map(|a| {
                let name = if Some(a.id) == active { format!("{} (active)", a.name) } else { a.name.clone() };
                vec![
                    name,
                    if a.authtoken.is_empty() { "—".into() } else { "set".into() },
                    if a.api_key.is_empty() { "—".into() } else { "set".into() },
                ]
            })
            .collect();
        let keep = self.selected.get().and_then(|id| cfg.accounts.iter().position(|a| a.id == id));
        self.table.set_rows(rows, keep);
    }

    fn select(&self, id: Option<Uuid>) {
        self.selected.set(id);
        let cfg = app().config.borrow().clone();
        let a = id.and_then(|id| cfg.account(id).cloned());
        let enabled = a.is_some();
        for f in [
            &*self.name as &objc2_app_kit::NSControl,
            &self.authtoken,
            &self.api_key,
            &self.server_addr,
            &self.proxy_url,
            &self.root_cas,
            &self.remove,
        ] {
            f.setEnabled(enabled);
        }
        let a = a.unwrap_or_else(|| Account::new(""));
        self.name.setStringValue(&ns(&a.name));
        self.authtoken.setStringValue(&ns(a.authtoken.expose()));
        self.api_key.setStringValue(&ns(a.api_key.expose()));
        self.server_addr.setStringValue(&ns(a.server_addr.as_deref().unwrap_or("")));
        self.proxy_url.setStringValue(&ns(a.proxy_url.as_deref().unwrap_or("")));
        self.root_cas.setStringValue(&ns(a.root_cas.as_deref().unwrap_or("")));
        self.status.setStringValue(&ns(""));
        if let Some(i) = id.and_then(|id| cfg.accounts.iter().position(|a| a.id == id)) {
            if self.table.selected() != Some(i) {
                let rows = cfg.accounts.len();
                let _ = rows;
                self.reload_list();
            }
        }
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
        app.config.borrow_mut().upsert_account(a);
        if app.save() {
            self.status.setStringValue(&ns("Saved. Running tunnels keep their current connection until restarted."));
            self.reload_list();
        }
    }

    fn validate(&self) {
        let Some(a) = self.collect() else { return };
        self.status.setStringValue(&ns("Checking…"));
        let status = self.status.clone();
        let rt = app().core.runtime().clone();
        let status = SendPtr(status);
        rt.spawn(async move {
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
        self.reload_list();
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
