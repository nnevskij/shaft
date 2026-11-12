//! Inspector pane: live list of captured HTTP exchanges, detail view,
//! replay (verbatim or edited), curl/HAR export.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::UNIX_EPOCH,
};

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSPopUpButton, NSSearchField, NSSwitch, NSTextField, NSTextView, NSView, NSWindow};
use shaft_core::InspectorEntry;
use shaft_inspector::{
    format::{self, BodyView},
    Exchange, ExchangeState, ReplayRequest,
};
use uuid::Uuid;

use super::main::Section;
use crate::{
    app::app,
    kit,
    platform::on_main,
    ui::{self, ns, Handlers, Table},
};

const METHODS: [&str; 8] = ["All Methods", "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const STATUSES: [&str; 5] = ["All Statuses", "2xx", "3xx", "4xx", "5xx / Errors"];
const VIEWS: [&str; 3] = ["Pretty", "Raw", "Hex"];

/// The "Edit & Replay" sheet, its text view and its handlers.
type ReplayEditor = (Retained<NSWindow>, Retained<NSTextView>, Handlers);

pub struct InspectorPane {
    pub view: Retained<NSView>,
    _handlers: Handlers,
    runs: RefCell<Vec<InspectorEntry>>,
    run_popup: Retained<NSPopUpButton>,
    current: Cell<Option<Uuid>>,
    filter: Retained<NSTextField>,
    method: Retained<NSPopUpButton>,
    status: Retained<NSPopUpButton>,
    view_mode: Retained<NSPopUpButton>,
    table: Table,
    /// Exchange ids in table order.
    shown: RefCell<Vec<u64>>,
    selected_id: Cell<Option<u64>>,
    detail: Retained<NSTextView>,
    summary: Retained<NSTextField>,
    replay: Retained<NSButton>,
    edit_replay: Retained<NSButton>,
    curl: Retained<NSButton>,
    follow: Retained<NSSwitch>,
    editor: RefCell<Option<ReplayEditor>>,
}

pub fn open(run: Option<Uuid>) {
    super::main::show(Section::Inspector);
    with(|w| {
        if run.is_some() {
            w.current.set(run);
        }
        w.reload_runs();
    });
}

pub fn pane() -> Retained<NSView> {
    let w = Rc::new(build());
    *app().windows.inspector.borrow_mut() = Some(w.clone());
    w.view.clone()
}

fn with(f: impl FnOnce(&InspectorPane)) {
    let w = app().windows.inspector.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn build() -> InspectorPane {
    let h = Handlers::new();

    let run_popup = ui::popup(&["No inspected tunnels"], 0);
    ui::on_action(&run_popup, &h, || {
        with(|w| {
            let i = ui::selected(&w.run_popup);
            let id = w.runs.borrow().get(i).map(|e| e.run_id);
            w.current.set(id);
            w.selected_id.set(None);
            w.reload_table();
        })
    });
    let search = NSSearchField::new(ui::mtm());
    search.setPlaceholderString(Some(&ns("Filter path, header or body")));
    ui::set_min_width(&search, 200.0);
    let filter: Retained<NSTextField> = Retained::into_super(search);
    ui::on_action(&filter, &h, || with(|w| w.reload_table()));
    let method = ui::popup_with(&METHODS, 0, &h, |_| with(|w| w.reload_table()));
    let status = ui::popup_with(&STATUSES, 0, &h, |_| with(|w| w.reload_table()));
    let follow = kit::switch(true);
    let follow_label = ui::secondary_label("Follow");
    let clear = kit::glass_button("", Some("trash"), false, &h, || {
        with(|w| {
            if let Some(e) = w.entry() {
                e.store.clear();
                w.selected_id.set(None);
                w.reload_table();
            }
        })
    });
    clear.setToolTip(Some(&ns("Clear captured requests")));
    let har = kit::glass_button("", Some("square.and.arrow.up"), false, &h, || with(|w| w.export_har()));
    har.setToolTip(Some(&ns("Export HAR…")));
    let toolbar = ui::fill_hstack(&[&run_popup, &filter, &method, &status, &follow_label, &follow, &clear, &har], 10.0);

    let table =
        Table::new(&[("Time", 76.0), ("Method", 64.0), ("Path", 380.0), ("Status", 56.0), ("Duration", 76.0), ("Size", 76.0), ("", 70.0)]);
    table.set_full_width_style();
    ui::set_height(&table.scroll, 260.0);
    table.on_select(|i| {
        with(|w| {
            let id = i.and_then(|i| w.shown.borrow().get(i).copied());
            w.selected_id.set(id);
            w.show_detail();
        })
    });
    table.on_double_click(|_| with(|w| w.replay_selected()));
    let table_card = kit::card(&table.scroll, 0.0);

    let summary = ui::label("");
    summary.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize_weight(13.0, kit::weight::SEMIBOLD)));
    let view_mode = ui::popup_with(&VIEWS, 0, &h, |_| with(|w| w.show_detail()));
    let curl = kit::glass_button("curl", Some("terminal"), false, &h, || with(|w| w.copy_curl()));
    curl.setToolTip(Some(&ns("Copy as curl")));
    let edit_replay = kit::glass_button("Edit & Replay…", Some("square.and.pencil"), false, &h, || with(|w| w.open_editor()));
    let replay = kit::glass_button("Replay", Some("arrow.clockwise"), true, &h, || with(|w| w.replay_selected()));
    let detail_bar = ui::fill_hstack(&[&summary, &ui::spacer(), &view_mode, &curl, &edit_replay, &replay], 10.0);
    let (detail_scroll, detail) = ui::text_area("", true, false);
    detail_scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
    detail.setTextContainerInset(objc2_foundation::NSSize::new(8.0, 8.0));
    ui::set_min_height(&detail_scroll, 200.0);
    let detail_card = kit::card(&detail_scroll, 0.0);

    let root = ui::vstack(&[&toolbar, &table_card, &detail_bar, &detail_card], 14.0);
    for v in [&*toolbar as &NSView, &table_card, &detail_bar, &detail_card] {
        ui::fill_width(&root, v);
    }
    ui::pad(&root, 20.0, 24.0);
    detail_card.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Vertical);

    let w = InspectorPane {
        view: ui::v(&*root),
        _handlers: h,
        runs: RefCell::new(Vec::new()),
        run_popup,
        current: Cell::new(None),
        filter,
        method,
        status,
        view_mode,
        table,
        shown: RefCell::new(Vec::new()),
        selected_id: Cell::new(None),
        detail,
        summary,
        replay,
        edit_replay,
        curl,
        follow,
        editor: RefCell::new(None),
    };
    w.update_buttons();
    w
}

impl InspectorPane {
    fn entry(&self) -> Option<InspectorEntry> {
        let id = self.current.get()?;
        app().core.inspector(id)
    }

    /// Refresh the tunnel selector (runs started/stopped).
    pub fn reload_runs(&self) {
        let runs = app().core.inspectors();
        self.run_popup.removeAllItems();
        if runs.is_empty() {
            self.run_popup.addItemWithTitle(&ns("No inspected tunnels"));
            self.run_popup.setEnabled(false);
        } else {
            self.run_popup.setEnabled(true);
            for r in &runs {
                let label = format!("{}{} — {}", r.name, if r.active { "" } else { " (stopped)" }, r.public_url.as_deref().unwrap_or(""));
                self.run_popup.addItemWithTitle(&ns(&label));
            }
        }
        let current = self.current.get().filter(|id| runs.iter().any(|r| r.run_id == *id)).or_else(|| runs.first().map(|r| r.run_id));
        self.current.set(current);
        if let Some(i) = current.and_then(|id| runs.iter().position(|r| r.run_id == id)) {
            self.run_popup.selectItemAtIndex(i as isize);
        }
        *self.runs.borrow_mut() = runs;
        self.reload_table();
    }

    pub fn traffic_changed(&self, run_id: Uuid) {
        if self.current.get() == Some(run_id) {
            self.reload_table();
        }
    }

    fn matches(&self, e: &Exchange) -> bool {
        let m = ui::selected(&self.method);
        if m > 0 && !e.method.eq_ignore_ascii_case(METHODS[m]) {
            return false;
        }
        let s = ui::selected(&self.status);
        if s > 0 {
            let ok = match (s, e.status) {
                (1, Some(c)) => (200..300).contains(&c),
                (2, Some(c)) => (300..400).contains(&c),
                (3, Some(c)) => (400..500).contains(&c),
                (4, c) => e.state == ExchangeState::Failed || c.map(|c| c >= 500).unwrap_or(false),
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        let q = ui::value(&self.filter).to_lowercase();
        if q.is_empty() {
            return true;
        }
        e.uri.to_lowercase().contains(&q)
            || e.request_headers
                .iter()
                .chain(&e.response_headers)
                .any(|(k, v)| k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q))
            || String::from_utf8_lossy(&e.request_body.bytes).to_lowercase().contains(&q)
            || String::from_utf8_lossy(&e.response_body.bytes).to_lowercase().contains(&q)
    }

    fn reload_table(&self) {
        let exchanges: Vec<Exchange> = self.entry().map(|e| e.store.all()).unwrap_or_default();
        let filtered: Vec<&Exchange> = exchanges.iter().filter(|e| self.matches(e)).collect();
        let rows = filtered
            .iter()
            .map(|e| {
                let t = e.started.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                let time = local_time(t);
                let status = match (e.state, e.status) {
                    (ExchangeState::Pending, _) => "…".into(),
                    (ExchangeState::Failed, None) => "ERR".into(),
                    (_, Some(s)) => s.to_string(),
                    _ => String::new(),
                };
                let tag = match (e.replay_of, e.state) {
                    (Some(_), _) => "replay".to_string(),
                    (_, ExchangeState::Upgraded) => "upgrade".into(),
                    (_, ExchangeState::Streaming) => "streaming".into(),
                    _ => String::new(),
                };
                vec![
                    time,
                    e.method.clone(),
                    e.uri.clone(),
                    status,
                    e.duration.map(format::format_duration).unwrap_or_default(),
                    format::format_size(e.response_body.total),
                    tag,
                ]
            })
            .collect();
        let ids: Vec<u64> = filtered.iter().map(|e| e.id).collect();
        let keep = self.selected_id.get().and_then(|id| ids.iter().position(|x| *x == id));
        let grew = ids.len() > self.shown.borrow().len();
        *self.shown.borrow_mut() = ids;
        self.table.set_rows(rows, keep);
        if keep.is_none() {
            self.selected_id.set(None);
        }
        if grew && kit::is_on(&self.follow) {
            self.table.scroll_to_end();
        }
        let counters = self.entry().map(|e| e.store.counters()).unwrap_or_default();
        super::main::set_subtitle(
            Section::Inspector,
            &format!("{} requests · {} upstream errors", counters.total, counters.upstream_errors),
        );
        self.show_detail();
    }

    fn selected_exchange(&self) -> Option<Exchange> {
        let id = self.selected_id.get()?;
        self.entry()?.store.get(id)
    }

    fn update_buttons(&self) {
        let has = self.selected_id.get().is_some();
        self.replay.setEnabled(has);
        self.edit_replay.setEnabled(has);
        self.curl.setEnabled(has);
    }

    fn show_detail(&self) {
        self.update_buttons();
        let Some(e) = self.selected_exchange() else {
            self.summary.setStringValue(&ns(""));
            self.detail.setString(&ns(if self.current.get().is_none() {
                "Start an HTTP tunnel with “Capture traffic” enabled to see requests here."
            } else {
                "Select a request."
            }));
            return;
        };
        let view = match ui::selected(&self.view_mode) {
            1 => BodyView::Raw,
            2 => BodyView::Hex,
            _ => BodyView::Auto,
        };
        self.summary.setStringValue(&ns(&format!(
            "#{} {} {} → {}{}",
            e.id,
            e.method,
            e.uri,
            e.status.map(|s| s.to_string()).unwrap_or_else(|| "—".into()),
            e.duration.map(|d| format!(" in {}", format::format_duration(d))).unwrap_or_default()
        )));
        self.detail.setString(&ns(&render_exchange(&e, view)));
    }

    fn base_url(&self) -> String {
        self.entry().and_then(|e| e.public_url).unwrap_or_else(|| "http://localhost".into())
    }

    fn copy_curl(&self) {
        if let Some(e) = self.selected_exchange() {
            ui::copy_to_clipboard(&format::to_curl(&e, &self.base_url()));
        }
    }

    fn export_har(&self) {
        let Some(entry) = self.entry() else { return };
        let all = entry.store.all();
        if all.is_empty() {
            return ui::info("Nothing to export", "No requests have been captured for this tunnel.");
        }
        let har = format::to_har(&all, &self.base_url());
        let Some(path) = ui::save_panel(&format!("{}.har", entry.name.replace(['/', ' '], "-"))) else { return };
        match serde_json::to_vec_pretty(&har).map_err(|e| e.to_string()).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())) {
            Ok(()) => ui::reveal_in_finder(&path),
            Err(e) => ui::error("Export failed", &e),
        }
    }

    fn send_replay(&self, req: ReplayRequest, replay_of: Option<u64>) {
        let Some(run) = self.current.get() else { return };
        app().core.replay(run, req, replay_of, move |res| {
            on_main(move || {
                with(|w| match res {
                    Ok(id) => {
                        w.selected_id.set(Some(id));
                        w.reload_table();
                    }
                    Err(e) => ui::error("Replay failed", &e),
                })
            })
        });
    }

    fn replay_selected(&self) {
        let Some(e) = self.selected_exchange() else { return };
        match ReplayRequest::from_exchange(&e) {
            Ok(req) => self.send_replay(req, Some(e.id)),
            Err(msg) => {
                if ui::confirm("Cannot replay verbatim", &format!("{msg}.\n\nEdit the request before replaying?"), "Edit…", false) {
                    self.open_editor();
                }
            }
        }
    }

    fn open_editor(&self) {
        let Some(e) = self.selected_exchange() else { return };
        let req = ReplayRequest {
            method: e.method.clone(),
            path: e.uri.clone(),
            headers: e.request_headers.clone(),
            body: e.request_body.bytes.clone(),
        };
        let h = Handlers::new();
        let win = ui::window(&format!("Edit & Replay #{}", e.id), 640.0, 480.0, true);
        let note = ui::secondary_label("Edit the request line, headers and body. It is sent directly to the upstream, not through ngrok.");
        let (scroll, text) = ui::text_area(&format::to_raw_request(&req), true, true);
        ui::set_min_height(&scroll, 320.0);
        let original = e.id;
        let send = kit::glass_button("Send", Some("paperplane.fill"), true, &h, move || {
            with(|w| {
                let Some((win, text, _)) = w.editor.borrow().as_ref().map(|(a, b, _)| (a.clone(), b.clone(), ())) else { return };
                match format::parse_raw_request(&ui::text_of(&text)) {
                    Ok(req) => {
                        end_sheet(&win);
                        w.send_replay(req, Some(original));
                    }
                    Err(e) => ui::error("Invalid request", &e),
                }
            })
        });
        send.setKeyEquivalent(&ns("\r"));
        let cancel = kit::glass_button("Cancel", None, false, &h, || {
            with(|w| {
                let win = w.editor.borrow().as_ref().map(|(win, _, _)| win.clone());
                if let Some(win) = win {
                    end_sheet(&win);
                }
            })
        });
        let buttons = ui::hstack(&[&ui::spacer(), &cancel, &send], 10.0);
        let root = ui::vstack(&[&note, &scroll, &buttons], 14.0);
        ui::pad(&root, 4.0, 4.0);
        ui::fill_width(&root, &scroll);
        ui::fill_width(&root, &buttons);
        ui::set_window_content(&win, &root);
        if let Some((old, _, _)) = self.editor.replace(Some((win.clone(), text, h))) {
            end_sheet(&old);
        }
        match super::main::window() {
            Some(parent) => parent.beginSheet_completionHandler(&win, None),
            None => ui::show_window(&win),
        }
    }
}

fn local_time(epoch_secs: u64) -> String {
    let t = epoch_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn render_exchange(e: &Exchange, view: BodyView) -> String {
    let mut s = String::new();
    if let Some(of) = e.replay_of {
        s.push_str(&format!("(replay of #{of})\n"));
    }
    if let Some(addr) = e.remote_addr {
        s.push_str(&format!("Client: {addr}\n"));
    }
    if let Some(err) = &e.error {
        s.push_str(&format!("Error: {err}\n"));
    }
    s.push_str(&format!("\n── REQUEST ─────────────────────────────\n{} {} {}\n", e.method, e.uri, e.version));
    for (k, v) in &e.request_headers {
        s.push_str(&format!("{k}: {v}\n"));
    }
    s.push('\n');
    s.push_str(&body_text(&e.request_headers, &e.request_body, view));
    s.push_str("\n\n── RESPONSE ────────────────────────────\n");
    match e.status {
        Some(code) => {
            let reason = http_reason(code);
            s.push_str(&format!("{code} {reason}\n"));
            for (k, v) in &e.response_headers {
                s.push_str(&format!("{k}: {v}\n"));
            }
            s.push('\n');
            match e.state {
                ExchangeState::Upgraded => s.push_str("[connection upgraded — frames are not captured]"),
                ExchangeState::Streaming => s.push_str(&format!("[streaming… {} so far]", format::format_size(e.response_body.total))),
                _ => s.push_str(&body_text(&e.response_headers, &e.response_body, view)),
            }
        }
        None => s.push_str("[waiting for upstream…]"),
    }
    s
}

fn body_text(headers: &[(String, String)], b: &shaft_inspector::CapturedBody, view: BodyView) -> String {
    if b.total == 0 {
        return "[empty body]".into();
    }
    let mut out = format::render_body(headers, &b.bytes, view);
    if b.truncated {
        out.push_str(&format!(
            "\n\n[truncated: showing {} of {}]",
            format::format_size(b.bytes.len() as u64),
            format::format_size(b.total)
        ));
    }
    if !b.complete {
        out.push_str("\n[body incomplete]");
    }
    out
}

fn http_reason(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// Close a sheet (or a plain window when shown without a parent).
fn end_sheet(win: &NSWindow) {
    match win.sheetParent() {
        Some(parent) => parent.endSheet(win),
        None => win.close(),
    }
}
