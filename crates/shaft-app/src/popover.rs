//! The popover shown from the menu bar icon. Rebuilt from state on each render.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSStackView, NSView};
use shaft_config::{Protocol, ProtocolOptions};
use shaft_core::{TunnelSnapshot, TunnelState};
use uuid::Uuid;

use crate::{
    app::{app, App},
    ui::{self, ns, Handlers},
    windows,
};

const WIDTH: f64 = 380.0;

pub fn render(app: &Rc<App>) {
    // Keep what the user typed in the quick field.
    if let Some(f) = app.quick_field.borrow_mut().take() {
        app.quick.borrow_mut().upstream = ui::value(&f);
    }
    app.popover_handlers.clear();
    let h = &app.popover_handlers;

    let content = if app.config.borrow().accounts.is_empty() { onboarding(app, h) } else { main_view(app, h) };
    // Pad via a wrapper so the content's children can fill its full width.
    let root = ui::vstack(&[&content], 0.0);
    ui::pad(&root, 14.0, 14.0);
    ui::fill_width(&root, &content);
    ui::set_width(&root, WIDTH);
    app.popover_vc.setView(&root);
    root.layoutSubtreeIfNeeded();
    let size = root.fittingSize();
    app.popover.setContentSize(objc2_foundation::NSSize::new(WIDTH, size.height.min(720.0)));
    ui::debug_snapshot("popover", &root);
}

fn onboarding(_a: &Rc<App>, h: &Handlers) -> Retained<NSStackView> {
    let title = ui::title_label("Welcome to Shaft");
    let intro = ui::wrapping_label(
        "Paste your ngrok authtoken to get started. You can find it in the ngrok dashboard under “Your Authtoken”. It is stored in your Keychain.",
        WIDTH - 28.0,
    );
    let name = ui::text_field("Personal", "Account name");
    let token = ui::secure_field("", "Authtoken");
    let status = ui::secondary_label("");
    let name_c = name.clone();
    let token_c = token.clone();
    let status_c = status.clone();
    let connect = ui::button("Connect", h, move || {
        let tok = token_c.stringValue().to_string();
        if tok.trim().is_empty() {
            status_c.setStringValue(&ns("Enter an authtoken."));
            return;
        }
        let account = crate::app::new_account(&ui::value(&name_c), &tok);
        status_c.setStringValue(&ns("Checking authtoken…"));
        windows::accounts::validate_and_add(account, status_c.clone());
    });
    connect.setKeyEquivalent(&ns("\r"));
    let dashboard = ui::link_button("Open dashboard ↗", h, || ui::open_url("https://dashboard.ngrok.com/get-started/your-authtoken"));
    let import = ui::button("Import ngrok.yml…", h, || windows::accounts::import_ngrok_yml(None));
    let quit = ui::small_button("Quit", h, || app().quit());

    let buttons = ui::hstack(&[&connect, &import, &ui::spacer(), &quit], 8.0);
    let s = ui::vstack(&[&title, &intro, &name, &token, &buttons, &status, &dashboard], 10.0);
    for v in [&*name as &NSView, &token, &buttons] {
        ui::fill_width(&s, v);
    }
    s
}

fn main_view(a: &Rc<App>, h: &Handlers) -> Retained<NSStackView> {
    let cfg = a.config.borrow().clone();
    let snaps = a.core.snapshots();
    let mut views: Vec<Retained<NSView>> = Vec::new();

    // Header: title + account selector.
    let names: Vec<String> = cfg.accounts.iter().map(|a| a.name.clone()).collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let active = cfg.active_account().map(|a| a.id);
    let sel = cfg.accounts.iter().position(|a| Some(a.id) == active).unwrap_or(0);
    let ids: Vec<Uuid> = cfg.accounts.iter().map(|a| a.id).collect();
    let account_popup = ui::popup_with(&name_refs, sel, h, move |i| {
        let app = app();
        app.config.borrow_mut().settings.active_account = ids.get(i).copied();
        app.save();
    });
    account_popup.setToolTip(Some(&ns("Account used for quick tunnels and the Account window")));
    let header = ui::hstack(&[&ui::title_label("Shaft"), &ui::spacer(), &account_popup], 8.0);
    views.push(ui::v(&*header));

    // Quick tunnel.
    views.push(ui::v(&*ui::section_label("Quick tunnel")));
    let q = a.quick.borrow().clone();
    let protos: Vec<&str> = Protocol::ALL.iter().map(|p| p.label()).collect();
    let proto_popup = ui::popup_with(&protos, Protocol::ALL.iter().position(|p| *p == q.protocol).unwrap_or(0), h, |i| {
        app().quick.borrow_mut().protocol = Protocol::ALL[i];
    });
    let field = ui::text_field(&q.upstream, "3000, host:port or URL");
    let start = ui::button("Start", h, || app().start_quick());
    let field_c = field.clone();
    ui::on_action(&field, h, move || {
        app().quick.borrow_mut().upstream = ui::value(&field_c);
        app().start_quick();
    });
    *a.quick_field.borrow_mut() = Some(field.clone());
    let quick = ui::fill_hstack(&[&proto_popup, &field, &start], 6.0);
    views.push(ui::v(&*quick));
    if let Some((name, e)) = a.quick_error.borrow().as_ref() {
        let mut e = e.clone();
        e.message = format!("{name}: {}", e.message);
        views.push(error_label(&e));
    }

    // Active tunnels.
    views.push(ui::v(&*ui::section_label(&format!("Active ({})", snaps.len()))));
    if snaps.is_empty() {
        views.push(ui::v(&*ui::secondary_label("No active tunnels.")));
    }
    for s in &snaps {
        views.push(tunnel_row(s, h));
    }

    // Profiles.
    views.push(ui::v(&*ui::section_label("Profiles")));
    let mut profiles = cfg.profiles.clone();
    profiles.sort_by_key(|p| (!p.favorite, p.name.to_lowercase()));
    if profiles.is_empty() {
        views.push(ui::v(&*ui::secondary_label("Save tunnels you use often as profiles.")));
    }
    for p in &profiles {
        let running = snaps.iter().any(|s| s.profile_id == p.id);
        let pid = p.id;
        let star = ui::icon_button(if p.favorite { "star.fill" } else { "star" }, "Favorite", h, move || {
            let app = app();
            if let Some(p) = app.config.borrow_mut().profile_mut(pid) {
                p.favorite = !p.favorite;
            }
            app.save();
        });
        let acc = cfg.account(p.account_id).map(|a| a.name.as_str()).unwrap_or("?");
        let target = match &p.options {
            ProtocolOptions::Http(_) => format!("http → {}", p.upstream),
            ProtocolOptions::Tcp(_) => format!("tcp → {}", p.upstream),
            ProtocolOptions::Tls(_) => format!("tls → {}", p.upstream),
        };
        let subtitle = if cfg.accounts.len() > 1 { format!("{target} · {acc}") } else { target };
        let name = ui::label(&p.name);
        let sub = ui::secondary_label(&subtitle);
        ui::resist_compression(&name);
        ui::resist_compression(&sub);
        let texts = ui::vstack(&[&name, &sub], 1.0);
        let toggle = ui::small_button(if running { "Stop" } else { "Start" }, h, move || app().toggle_profile(pid));
        let edit = ui::icon_button("slider.horizontal.3", "Edit profile", h, move || windows::profile::open(Some(pid)));
        let row = ui::hstack(&[&star, &texts, &ui::spacer(), &toggle, &edit], 6.0);
        views.push(ui::v(&*row));
        if !running {
            if let Some(e) = a.failures.borrow().get(&pid) {
                views.push(error_label(e));
            }
        }
    }
    let new_profile = ui::small_button("New profile…", h, || windows::profile::open(None));
    let stop_all = ui::small_button("Stop all", h, || app().core.stop_all());
    stop_all.setEnabled(!snaps.is_empty());
    views.push(ui::v(&*ui::hstack(&[&new_profile, &ui::spacer(), &stop_all], 6.0)));

    // Footer.
    views.push(ui::v(&*ui::separator()));
    let inspector = ui::icon_button("list.bullet.rectangle", "Inspector", h, || windows::inspector::open(None));
    let resources = ui::icon_button("globe", "ngrok resources (domains, endpoints, …)", h, windows::resources::open);
    let accounts = ui::icon_button("person.crop.circle", "Accounts", h, windows::accounts::open);
    let diag = ui::icon_button("stethoscope", "Diagnostics", h, windows::diagnostics::open);
    let settings = ui::icon_button("gearshape", "Settings", h, windows::settings::open);
    let quit = ui::small_button("Quit", h, || app().quit());
    let footer = ui::hstack(&[&inspector, &resources, &accounts, &diag, &settings, &ui::spacer(), &quit], 10.0);
    views.push(ui::v(&*footer));

    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let stack = ui::vstack(&refs, 8.0);
    for v in &views {
        ui::fill_width(&stack, v);
    }
    stack
}

fn error_label(e: &shaft_core::CoreError) -> Retained<NSView> {
    let mut text = format!("⚠ {e}");
    if let Some(hint) = e.hint() {
        text.push_str(&format!("\n{hint}"));
    }
    let l = ui::wrapping_label(&text, WIDTH - 40.0);
    l.setTextColor(Some(&NSColor::systemRedColor()));
    l.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(11.0)));
    ui::v(&*l)
}

fn tunnel_row(s: &TunnelSnapshot, h: &Handlers) -> Retained<NSView> {
    let run = s.run_id;
    let (dot_color, state_text) = match &s.state {
        TunnelState::Online => (NSColor::systemGreenColor(), "Online".to_string()),
        TunnelState::Starting => (NSColor::systemYellowColor(), "Starting…".into()),
        TunnelState::Reconnecting { attempt } => (NSColor::systemOrangeColor(), format!("Reconnecting (#{attempt})…")),
        TunnelState::Failed(_) => (NSColor::systemRedColor(), "Error".into()),
        TunnelState::Stopping => (NSColor::secondaryLabelColor(), "Stopping…".into()),
    };
    let dot = ui::colored_label("●", &dot_color);
    let name = ui::label(&s.name);
    ui::resist_compression(&name);
    let mut meta = format!("{} · {}", s.protocol.label(), state_text);
    if s.inspect {
        meta.push_str(&format!(" · {} req", s.requests));
    }
    let meta = ui::secondary_label(&meta);
    let stop = ui::icon_button("stop.circle", "Stop", h, move || app().core.stop(run));
    let top = ui::hstack(&[&dot, &name, &meta, &ui::spacer(), &stop], 6.0);

    let mut rows: Vec<Retained<NSView>> = vec![ui::v(&*top)];
    if let Some(url) = &s.public_url {
        let u = url.clone();
        let link = ui::link_button(url, h, move || app().copy_url(&u));
        link.setToolTip(Some(&ns("Click to copy")));
        let u2 = url.clone();
        let open = ui::icon_button("safari", "Open in browser", h, move || ui::open_url(&u2));
        let mut items: Vec<Retained<NSView>> = vec![ui::v(&*link), ui::v(&*ui::spacer())];
        if s.inspect {
            let inspect = ui::icon_button("list.bullet.rectangle", "Inspect traffic", h, move || windows::inspector::open(Some(run)));
            items.push(ui::v(&*inspect));
        }
        items.push(ui::v(&*open));
        let refs: Vec<&NSView> = items.iter().map(|v| &**v).collect();
        rows.push(ui::v(&*ui::hstack(&refs, 6.0)));
    }
    rows.push(ui::v(&*ui::secondary_label(&format!("→ {}", s.upstream))));
    if s.upstream_reachable == Some(false) {
        let l = ui::colored_label("⚠ Upstream unreachable — is your local service running?", &NSColor::systemOrangeColor());
        rows.push(ui::v(&*l));
    }
    if let TunnelState::Failed(e) = &s.state {
        rows.push(error_label(e));
    }
    let refs: Vec<&NSView> = rows.iter().map(|v| &**v).collect();
    let block = ui::vstack(&refs, 2.0);
    for r in &rows {
        ui::fill_width(&block, r);
    }
    ui::v(&*block)
}
