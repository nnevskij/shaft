//! ngrok Resources pane over the REST API (`ngrok api …` parity):
//! catalogued resources with list/create/update/delete/actions, plus a
//! generic API explorer for everything else.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSPopUpButton, NSStackView, NSTextField, NSTextView, NSView};
use serde_json::Value;
use shaft_api::{cell, ApiClient, Method, ResourceKind, RESOURCES};
use shaft_config::{Protocol, TunnelProfile};
use uuid::Uuid;

use super::main::Section;
use crate::{
    app::app,
    kit,
    platform::on_main,
    ui::{self, ns, Handlers, Table},
};

const EXPLORER: &str = "API Explorer";
const HTTP_METHODS: [&str; 5] = ["GET", "POST", "PATCH", "PUT", "DELETE"];

pub struct ResourcesPane {
    pub view: Retained<NSView>,
    _handlers: Handlers,
    account_ids: RefCell<Vec<Uuid>>,
    account: Retained<NSPopUpButton>,
    _kinds: Table,
    kind_index: Cell<usize>,
    title: Retained<NSTextField>,
    status: Retained<NSTextField>,
    // Resource mode
    resource_box: Retained<NSStackView>,
    table_holder: Retained<NSStackView>,
    table: RefCell<Table>,
    items: RefCell<Vec<Value>>,
    editor: Retained<NSTextView>,
    /// None = editing a new item.
    editing: RefCell<Option<String>>,
    create: Retained<NSButton>,
    save: Retained<NSButton>,
    delete: Retained<NSButton>,
    use_domain: Retained<NSButton>,
    actions_box: Retained<NSStackView>,
    action_handlers: Handlers,
    // Explorer mode
    explorer_box: Retained<NSStackView>,
    ex_method: Retained<NSPopUpButton>,
    ex_path: Retained<NSTextField>,
    ex_body: Retained<NSTextView>,
    ex_result: Retained<NSTextView>,
    busy: Cell<bool>,
}

pub fn open() {
    super::main::show(Section::Resources);
}

pub fn pane() -> Retained<NSView> {
    let w = Rc::new(build());
    *app().windows.resources.borrow_mut() = Some(w.clone());
    w.reload_accounts();
    w.kind_changed();
    w.view.clone()
}

fn with(f: impl FnOnce(&ResourcesPane)) {
    let w = app().windows.resources.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn card_area(scroll: &NSView) -> Retained<NSView> {
    ui::v(&*kit::card(scroll, 0.0))
}

fn build() -> ResourcesPane {
    let h = Handlers::new();

    // Left: resource kinds.
    let kinds = Table::new(&[("Resource", 180.0)]);
    kinds.set_inset_style();
    let mut kind_rows: Vec<Vec<String>> = RESOURCES.iter().map(|r| vec![r.title.to_string()]).collect();
    kind_rows.push(vec![EXPLORER.to_string()]);
    kinds.set_rows(kind_rows, Some(0));
    kinds.on_select(|i| {
        with(|w| {
            if let Some(i) = i {
                if i != w.kind_index.get() {
                    w.kind_index.set(i);
                    w.kind_changed();
                }
            }
        })
    });
    let dashboard = ui::link_button("Open Dashboard ↗", &h, || ui::open_url("https://dashboard.ngrok.com"));
    let left = ui::vstack(&[&kinds.scroll, &dashboard], 12.0);
    ui::fill_width(&left, &kinds.scroll);
    kinds.scroll.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Vertical);

    // Right: header.
    let title = kit::pane_title("");
    let account = ui::popup(&[], 0);
    ui::on_action(&account, &h, || with(|w| w.kind_changed()));
    let refresh = kit::glass_button("", Some("arrow.clockwise"), false, &h, || with(|w| w.refresh()));
    refresh.setToolTip(Some(&ns("Refresh")));
    let header = ui::fill_hstack(&[&title, &ui::spacer(), &ui::secondary_label("Account"), &account, &refresh], 8.0);
    let status = kit::footnote("", 600.0);

    // Resource mode.
    let table = Table::new(&[("", 100.0)]);
    let table_holder = ui::vstack(&[&table.scroll], 0.0);
    ui::fill_width(&table_holder, &table.scroll);
    let table_card = kit::card(&table_holder, 0.0);
    ui::set_height(&table_holder, 250.0);
    let (editor_scroll, editor) = ui::text_area("", true, true);
    editor_scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
    editor.setTextContainerInset(objc2_foundation::NSSize::new(8.0, 8.0));
    ui::set_min_height(&editor_scroll, 180.0);
    let new = kit::glass_button("New", Some("plus"), false, &h, || with(|w| w.new_item()));
    let create = kit::glass_button("Create", None, true, &h, || with(|w| w.create_item()));
    let save = kit::glass_button("Save Changes", None, true, &h, || with(|w| w.update_item()));
    let delete = kit::glass_button("Delete…", Some("trash"), false, &h, || with(|w| w.delete_item()));
    let use_domain =
        kit::glass_button("New Tunnel Profile", Some("arrow.up.and.down.circle"), false, &h, || with(|w| w.profile_from_item()));
    let actions_box = ui::hstack(&[], 8.0);
    let item_bar = ui::hstack(&[&new, &use_domain, &actions_box, &ui::spacer(), &delete, &create, &save], 10.0);
    let json_label = ui::secondary_label("JSON — select an item to edit it, or press New");
    let editor_card = card_area(&editor_scroll);
    let resource_box = ui::vstack(&[&table_card, &item_bar, &json_label, &editor_card], 12.0);
    for v in [&*table_card as &NSView, &item_bar, &editor_card] {
        ui::fill_width(&resource_box, v);
    }

    // Explorer mode.
    let ex_method = ui::popup(&HTTP_METHODS, 0);
    let ex_path = ui::text_field("/endpoints", "/path?query (relative to https://api.ngrok.com)");
    kit::rounded(&ex_path);
    let send = kit::glass_button("Send", Some("paperplane.fill"), true, &h, || with(|w| w.explore()));
    let docs = ui::link_button("API Reference ↗", &h, || ui::open_url("https://ngrok.com/docs/api/"));
    let ex_bar = ui::fill_hstack(&[&ex_method, &ex_path, &send], 8.0);
    let (ex_body_scroll, ex_body) = ui::text_area("", true, true);
    ex_body_scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
    ui::set_height(&ex_body_scroll, 110.0);
    let (ex_result_scroll, ex_result) = ui::text_area("", true, false);
    ex_result_scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
    ui::set_min_height(&ex_result_scroll, 260.0);
    let body_card = card_area(&ex_body_scroll);
    let result_card = card_area(&ex_result_scroll);
    let explorer_box = ui::vstack(
        &[&ex_bar, &ui::secondary_label("JSON body (POST/PATCH/PUT)"), &body_card, &ui::secondary_label("Response"), &result_card, &docs],
        8.0,
    );
    for v in [&*ex_bar as &NSView, &body_card, &result_card] {
        ui::fill_width(&explorer_box, v);
    }

    let right = ui::vstack(&[&header, &resource_box, &explorer_box, &status], 18.0);
    for v in [&*header as &NSView, &resource_box, &explorer_box, &status] {
        ui::fill_width(&right, v);
    }

    let root = NSView::new(ui::mtm());
    for v in [&*left as &NSView, &right] {
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.addSubview(v);
    }
    left.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0).setActive(true);
    left.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 20.0).setActive(true);
    left.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -16.0).setActive(true);
    left.widthAnchor().constraintEqualToConstant(200.0).setActive(true);
    right.leadingAnchor().constraintEqualToAnchor_constant(&left.trailingAnchor(), 24.0).setActive(true);
    right.trailingAnchor().constraintEqualToAnchor_constant(&root.trailingAnchor(), -24.0).setActive(true);
    right.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 20.0).setActive(true);
    right.bottomAnchor().constraintLessThanOrEqualToAnchor_constant(&root.bottomAnchor(), -20.0).setActive(true);

    ResourcesPane {
        view: root,
        _handlers: h,
        account_ids: RefCell::new(Vec::new()),
        account,
        _kinds: kinds,
        kind_index: Cell::new(0),
        title,
        status,
        resource_box,
        table_holder,
        table: RefCell::new(table),
        items: RefCell::new(Vec::new()),
        editor,
        editing: RefCell::new(None),
        create,
        save,
        delete,
        use_domain,
        actions_box,
        action_handlers: Handlers::new(),
        explorer_box,
        ex_method,
        ex_path,
        ex_body,
        ex_result,
        busy: Cell::new(false),
    }
}

impl ResourcesPane {
    /// Called when the section is shown again.
    pub fn shown(&self) {
        self.reload_accounts();
        if self.items.borrow().is_empty() {
            self.kind_changed();
        }
    }

    fn reload_accounts(&self) {
        let cfg = app().config.borrow().clone();
        let prev = self.account_ids.borrow().get(ui::selected(&self.account)).copied();
        self.account.removeAllItems();
        let mut ids = Vec::new();
        for a in &cfg.accounts {
            let label = if a.api_key.is_empty() { format!("{} (no API key)", a.name) } else { a.name.clone() };
            self.account.addItemWithTitle(&ns(&label));
            ids.push(a.id);
        }
        let want = prev.or_else(|| cfg.active_account().map(|a| a.id));
        if let Some(i) = want.and_then(|id| ids.iter().position(|x| *x == id)) {
            self.account.selectItemAtIndex(i as isize);
        }
        *self.account_ids.borrow_mut() = ids;
    }

    fn client(&self) -> Option<ApiClient> {
        let id = *self.account_ids.borrow().get(ui::selected(&self.account))?;
        let acc = app().config.borrow().account(id).cloned()?;
        if acc.api_key.is_empty() {
            self.set_status("This account has no API key. Add one in Accounts to manage resources.");
            return None;
        }
        Some(ApiClient::new(acc.api_key.expose()))
    }

    fn kind(&self) -> Option<&'static ResourceKind> {
        RESOURCES.get(self.kind_index.get())
    }

    fn set_status(&self, s: &str) {
        self.status.setStringValue(&ns(s));
    }

    fn kind_changed(&self) {
        let kind = self.kind();
        self.title.setStringValue(&ns(kind.map(|k| k.title).unwrap_or(EXPLORER)));
        self.explorer_box.setHidden(kind.is_some());
        self.resource_box.setHidden(kind.is_none());
        let Some(kind) = kind else {
            self.set_status("");
            return;
        };
        // Rebuild the table with this resource's columns.
        let mut cols: Vec<(&str, f64)> = kind.columns.iter().map(|c| (c.title, 180.0)).collect();
        if let Some(last) = cols.last_mut() {
            last.1 = 160.0;
        }
        let table = Table::new(&cols);
        table.set_full_width_style();
        table.on_select(|i| with(|w| w.select(i)));
        for v in self.table_holder.arrangedSubviews().iter() {
            v.removeFromSuperview();
        }
        self.table_holder.addArrangedSubview(&table.scroll);
        ui::fill_width(&self.table_holder, &table.scroll);
        *self.table.borrow_mut() = table;
        self.items.borrow_mut().clear();
        self.editor.setString(&ns(""));
        *self.editing.borrow_mut() = None;
        self.create.setHidden(kind.create_template.is_none());
        self.use_domain.setHidden(!matches!(kind.id, "reserved_domains" | "reserved_addrs"));
        self.update_buttons();
        self.refresh();
    }

    fn update_buttons(&self) {
        let Some(kind) = self.kind() else { return };
        let editing_existing = self.editing.borrow().is_some();
        self.create.setEnabled(!editing_existing && kind.create_template.is_some());
        self.save.setHidden(!kind.updatable);
        self.save.setEnabled(editing_existing);
        self.delete.setHidden(!kind.deletable);
        self.delete.setEnabled(editing_existing);
        self.use_domain.setEnabled(editing_existing);
        self.action_handlers.clear();
        for v in self.actions_box.arrangedSubviews().iter() {
            v.removeFromSuperview();
        }
        for a in kind.actions {
            let action = *a;
            let b = kit::glass_button(a.label, None, false, &self.action_handlers, move || with(|w| w.run_action(action)));
            b.setEnabled(editing_existing);
            self.actions_box.addArrangedSubview(&b);
        }
    }

    /// Run an API call off the main thread and handle the result on it.
    fn call<T: Send + 'static>(
        &self,
        what: &str,
        fut: impl std::future::Future<Output = Result<T, shaft_api::ApiError>> + Send + 'static,
        done: impl FnOnce(&ResourcesPane, T) + Send + 'static,
    ) {
        if self.busy.replace(true) {
            return;
        }
        self.set_status(&format!("{what}…"));
        let rt = app().core.runtime().clone();
        rt.spawn(async move {
            let res = fut.await;
            on_main(move || {
                with(|w| {
                    w.busy.set(false);
                    match res {
                        Ok(v) => {
                            w.set_status("");
                            done(w, v);
                        }
                        Err(e) => {
                            let mut msg = e.to_string();
                            if e.is_forbidden() {
                                msg.push_str(" — your plan or API key may not allow this.");
                            }
                            if let Some(doc) = e.doc_url() {
                                msg.push_str(&format!(" ({doc})"));
                            }
                            w.set_status(&msg);
                        }
                    }
                })
            })
        });
    }

    fn refresh(&self) {
        let Some(kind) = self.kind() else { return };
        let Some(client) = self.client() else { return };
        self.call(&format!("Loading {}", kind.title.to_lowercase()), async move { client.list_all(kind).await }, move |w, items| {
            w.set_items(items);
            let n = w.items.borrow().len();
            w.set_status(&format!("{n} item(s)"));
        });
    }

    fn set_items(&self, items: Vec<Value>) {
        let Some(kind) = self.kind() else { return };
        let rows = items.iter().map(|it| kind.columns.iter().map(|c| cell(it, c.pointer)).collect()).collect();
        let keep_id = self.editing.borrow().clone();
        let keep = keep_id.and_then(|id| items.iter().position(|it| it.get("id").and_then(|v| v.as_str()) == Some(id.as_str())));
        *self.items.borrow_mut() = items;
        self.table.borrow().set_rows(rows, keep);
        if keep.is_none() {
            *self.editing.borrow_mut() = None;
        }
        self.update_buttons();
    }

    fn select(&self, i: Option<usize>) {
        let item = i.and_then(|i| self.items.borrow().get(i).cloned());
        match item {
            Some(it) => {
                *self.editing.borrow_mut() = it.get("id").and_then(|v| v.as_str()).map(str::to_string);
                self.editor.setString(&ns(&serde_json::to_string_pretty(&editable(&it)).unwrap_or_default()));
            }
            None => {
                *self.editing.borrow_mut() = None;
                self.editor.setString(&ns(""));
            }
        }
        self.update_buttons();
    }

    fn new_item(&self) {
        let Some(kind) = self.kind() else { return };
        let Some(t) = kind.create_template else { return };
        self.table.borrow().set_rows(self.table_rows(), None);
        *self.editing.borrow_mut() = None;
        self.editor.setString(&ns(t));
        self.update_buttons();
        self.set_status("Edit the JSON, then click Create.");
    }

    fn table_rows(&self) -> Vec<Vec<String>> {
        let Some(kind) = self.kind() else { return Vec::new() };
        self.items.borrow().iter().map(|it| kind.columns.iter().map(|c| cell(it, c.pointer)).collect()).collect()
    }

    fn editor_json(&self) -> Option<Value> {
        match serde_json::from_str::<Value>(&ui::text_of(&self.editor)) {
            Ok(v) if v.is_object() => Some(v),
            Ok(_) => {
                self.set_status("The body must be a JSON object.");
                None
            }
            Err(e) => {
                self.set_status(&format!("Invalid JSON: {e}"));
                None
            }
        }
    }

    fn create_item(&self) {
        let Some(kind) = self.kind() else { return };
        let Some(body) = self.editor_json() else { return };
        let Some(client) = self.client() else { return };
        self.call(
            &format!("Creating {}", kind.title.to_lowercase()),
            async move { client.create(kind, &body).await },
            move |w, created| {
                if let Some(field) = kind.secret_on_create {
                    if let Some(secret) = created.get(field).and_then(|v| v.as_str()) {
                        ui::copy_to_clipboard(secret);
                        ui::info(
                            "Copy this value now",
                            &format!("The new {field} was copied to the clipboard. ngrok shows it only once:\n\n{secret}"),
                        );
                    }
                }
                *w.editing.borrow_mut() = created.get("id").and_then(|v| v.as_str()).map(str::to_string);
                w.refresh();
            },
        );
    }

    fn update_item(&self) {
        let Some(kind) = self.kind() else { return };
        let Some(id) = self.editing.borrow().clone() else { return };
        let Some(mut body) = self.editor_json() else { return };
        strip_read_only(&mut body);
        let Some(client) = self.client() else { return };
        self.call("Saving", async move { client.update(kind, &id, &body).await }, |w, _| {
            w.refresh();
            w.set_status("Saved.");
        });
    }

    fn delete_item(&self) {
        let Some(kind) = self.kind() else { return };
        let Some(id) = self.editing.borrow().clone() else { return };
        let name = self
            .items
            .borrow()
            .iter()
            .find(|it| it.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
            .map(|it| cell(it, kind.name_pointer))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.clone());
        if !ui::confirm(
            &format!("Delete {} “{name}”?", kind.title.trim_end_matches('s').to_lowercase()),
            "This deletes the resource from your ngrok account. It cannot be undone.",
            "Delete",
            true,
        ) {
            return;
        }
        let Some(client) = self.client() else { return };
        self.call("Deleting", async move { client.delete(kind, &id).await }, |w, ()| {
            *w.editing.borrow_mut() = None;
            w.editor.setString(&ns(""));
            w.refresh();
        });
    }

    fn run_action(&self, action: shaft_api::ItemAction) {
        let Some(kind) = self.kind() else { return };
        let Some(id) = self.editing.borrow().clone() else { return };
        if action.destructive
            && !ui::confirm(&format!("{} {id}?", action.label), "The remote agent will be affected immediately.", action.label, true)
        {
            return;
        }
        let Some(client) = self.client() else { return };
        self.call(action.label, async move { client.action(kind, &id, action.suffix).await }, move |w, _| {
            w.set_status(&format!("{} requested.", action.label));
            w.refresh();
        });
    }

    fn profile_from_item(&self) {
        let Some(kind) = self.kind() else { return };
        let Some(id) = self.editing.borrow().clone() else { return };
        let item = self.items.borrow().iter().find(|it| it.get("id").and_then(|v| v.as_str()) == Some(id.as_str())).cloned();
        let Some(item) = item else { return };
        let account_id = match self.account_ids.borrow().get(ui::selected(&self.account)) {
            Some(id) => *id,
            None => return,
        };
        let (protocol, url) = match kind.id {
            "reserved_addrs" => (Protocol::Tcp, cell(&item, "/addr")),
            _ => (Protocol::Http, cell(&item, "/domain")),
        };
        let app = app();
        let mut p = TunnelProfile::new(url.split('.').next().unwrap_or("tunnel").to_string(), account_id, protocol, "");
        p.url = url;
        let base = p.name.clone();
        let mut n = 2;
        while app.config.borrow().profiles.iter().any(|o| o.name.eq_ignore_ascii_case(&p.name)) {
            p.name = format!("{base}-{n}");
            n += 1;
        }
        // Saved without an upstream: the editor asks for it before it can be used.
        let id = p.id;
        app.config.borrow_mut().upsert_profile(p);
        if app.save() {
            super::profile::open(Some(id));
        }
    }

    fn explore(&self) {
        let method = HTTP_METHODS[ui::selected(&self.ex_method).min(HTTP_METHODS.len() - 1)];
        let path = ui::value(&self.ex_path);
        if path.is_empty() {
            return;
        }
        let body_text = ui::text_of(&self.ex_body);
        let body = if body_text.trim().is_empty() || method == "GET" || method == "DELETE" {
            None
        } else {
            match serde_json::from_str::<Value>(&body_text) {
                Ok(v) => Some(v),
                Err(e) => return self.set_status(&format!("Invalid JSON body: {e}")),
            }
        };
        if matches!(method, "DELETE" | "POST" | "PATCH" | "PUT")
            && !ui::confirm(&format!("{method} {path}?"), "This request may modify your ngrok account.", "Send", method == "DELETE")
        {
            return;
        }
        let Some(client) = self.client() else { return };
        let m: Method = method.parse().unwrap_or(Method::GET);
        self.call(&format!("{method} {path}"), async move { client.request(m, &path, body.as_ref()).await }, |w, v| {
            w.ex_result.setString(&ns(&serde_json::to_string_pretty(&v).unwrap_or_default()));
        });
    }
}

/// Drop server-managed fields so the JSON can be PATCHed back.
fn editable(item: &Value) -> Value {
    item.clone()
}

fn strip_read_only(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        for k in
            ["id", "uri", "created_at", "updated_at", "cname_target", "acme_challenge_cname_target", "token", "owner_id", "addr", "region"]
        {
            o.remove(k);
        }
    }
}
