//! The menu bar popover, Control Center style: rounded modules on the
//! system popover material (Liquid Glass on macOS 26, vibrancy before).

use std::{cell::RefCell, rc::Rc};

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont, NSSegmentSwitchTracking, NSSegmentedControl, NSStackView, NSView};
use objc2_foundation::NSArray;
use shaft_config::Protocol;
use shaft_core::{TunnelSnapshot, TunnelState};
use uuid::Uuid;

use crate::{
    app::{app, App},
    kit::{self, weight, MenuEntry},
    ui::{self, ns, Handlers},
    windows::{self, main::Section},
};

const WIDTH: f64 = 360.0;
const PAD: f64 = 14.0;

thread_local! {
    /// Fixed container installed once as the popover's view (replacing the
    /// view of a shown popover makes AppKit re-attach it, which flickers),
    /// plus the signature of what it currently shows.
    static ROOT: RefCell<Option<(Retained<NSView>, String)>> = const { RefCell::new(None) };
}

/// Everything the popover displays. Renders are skipped when it is unchanged,
/// so frequent core events (probes, counters) do not rebuild the view.
fn signature(app: &App) -> String {
    let cfg = app.config.borrow();
    let accounts: Vec<_> = cfg.accounts.iter().map(|a| (a.id, &a.name)).collect();
    let profiles: Vec<_> = cfg.profiles.iter().map(|p| (p.id, &p.name, p.favorite, &p.upstream, p.account_id, p.protocol())).collect();
    let mut failures: Vec<_> = app.failures.borrow().iter().map(|(k, v)| (*k, v.clone())).collect();
    failures.sort_by_key(|(k, _)| *k);
    format!(
        "{accounts:?}|{:?}|{profiles:?}|{:?}|{failures:?}|{:?}|{:?}",
        cfg.active_account().map(|a| a.id),
        app.core.snapshots(),
        app.quick_error.borrow(),
        app.quick.borrow().protocol,
    )
}

pub fn render(app: &Rc<App>) {
    let sig = signature(app);
    let unchanged = ROOT.with(|r| r.borrow().as_ref().map(|(_, s)| *s == sig).unwrap_or(false));
    if unchanged {
        return;
    }
    tracing::trace!("popover render");

    // Keep what the user typed in the quick field, and whether it had focus.
    let mut quick_focused = false;
    if let Some(f) = app.quick_field.borrow_mut().take() {
        app.quick.borrow_mut().upstream = ui::value(&f);
        quick_focused = f.currentEditor().is_some();
    }
    app.popover_handlers.clear();
    let h = &app.popover_handlers;

    let root = ROOT.with(|r| {
        let mut r = r.borrow_mut();
        let root = match r.as_ref() {
            Some((v, _)) => v.clone(),
            None => {
                let v = NSView::new(ui::mtm());
                app.popover_vc.setView(&v);
                v
            }
        };
        *r = Some((root.clone(), sig));
        root
    });

    let content = if app.config.borrow().accounts.is_empty() { onboarding(h) } else { main_view(app, h) };
    // Pad via a wrapper so the content's children can fill its full width.
    let wrapper = ui::vstack(&[&content], 0.0);
    ui::pad(&wrapper, PAD, PAD);
    ui::fill_width(&wrapper, &content);
    ui::set_width(&wrapper, WIDTH);
    wrapper.layoutSubtreeIfNeeded();
    let height = wrapper.fittingSize().height.min(720.0);

    // Swap the content in place, without animation.
    for old in root.subviews().iter() {
        old.removeFromSuperview();
    }
    wrapper.setTranslatesAutoresizingMaskIntoConstraints(false);
    root.addSubview(&wrapper);
    wrapper.topAnchor().constraintEqualToAnchor(&root.topAnchor()).setActive(true);
    wrapper.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()).setActive(true);
    let current = app.popover.contentSize();
    if (current.height - height).abs() > 0.5 || (current.width - WIDTH).abs() > 0.5 {
        let animates = app.popover.animates();
        app.popover.setAnimates(false);
        app.popover.setContentSize(objc2_foundation::NSSize::new(WIDTH, height));
        app.popover.setAnimates(animates);
    }

    if quick_focused {
        if let (Some(f), Some(w)) = (app.quick_field.borrow().as_ref(), root.window()) {
            w.makeFirstResponder(Some(f));
            if let Some(editor) = f.currentEditor() {
                let len = f.stringValue().length();
                editor.setSelectedRange(objc2_foundation::NSRange::new(len, 0));
            }
        }
    }
    ui::debug_snapshot("popover", &wrapper);
}

/// Stack `views` vertically, each filling the width.
fn column(views: &[Retained<NSView>], spacing: f64) -> Retained<NSStackView> {
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let s = ui::vstack(&refs, spacing);
    for v in views {
        ui::fill_width(&s, v);
    }
    s
}

fn section_title(text: &str) -> Retained<NSView> {
    let l = ui::label(text);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, weight::SEMIBOLD)));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let s = ui::hstack(&[&l], 0.0);
    ui::pad_edges(&s, 2.0, kit::space::CARD_H, 0.0, kit::space::CARD_H);
    ui::v(&*s)
}

fn header(subtitle: &str, trailing: Option<&NSView>) -> Retained<NSView> {
    let tile = kit::icon_tile("arrow.up.and.down", &NSColor::systemIndigoColor(), 30.0);
    let title = ui::label("Shaft");
    title.setFont(Some(&NSFont::systemFontOfSize_weight(14.0, weight::SEMIBOLD)));
    let sub = ui::secondary_label(subtitle);
    let texts = ui::vstack(&[&title, &sub], 1.0);
    let mut items: Vec<&NSView> = vec![&tile, &texts];
    let spacer = ui::spacer();
    items.push(&spacer);
    if let Some(t) = trailing {
        items.push(t);
    }
    ui::v(&*ui::fill_hstack(&items, 10.0))
}

fn error_label(e: &shaft_core::CoreError) -> Retained<NSView> {
    let mut text = e.to_string();
    if let Some(hint) = e.hint() {
        text.push_str(&format!("\n{hint}"));
    }
    let icon = kit::symbol_view("exclamationmark.triangle.fill", 11.0, Some(&NSColor::systemRedColor()));
    let l = ui::wrapping_label(&text, WIDTH - 80.0);
    l.setTextColor(Some(&NSColor::systemRedColor()));
    l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    let row = ui::hstack(&[&icon, &l], 6.0);
    row.setAlignment(objc2_app_kit::NSLayoutAttribute::Top);
    ui::v(&*row)
}

// ---------------------------------------------------------------------------
// Onboarding
// ---------------------------------------------------------------------------

fn onboarding(h: &Handlers) -> Retained<NSStackView> {
    let intro =
        kit::footnote("Paste your ngrok authtoken to get started. It is stored in your Keychain, never on disk.", WIDTH - 2.0 * PAD - 24.0);
    let name = ui::text_field("Personal", "Account name");
    let token = ui::secure_field("", "Authtoken");
    let status = ui::secondary_label("");
    let name_c = name.clone();
    let token_c = token.clone();
    let status_c = status.clone();
    let connect = kit::glass_button("Connect", Some("bolt.horizontal.fill"), true, h, move || {
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
    let get = ui::link_button("Get your authtoken ↗", h, || ui::open_url("https://dashboard.ngrok.com/get-started/your-authtoken"));
    let form = column(
        &[ui::v(&*intro), ui::v(&*name), ui::v(&*token), ui::v(&*ui::hstack(&[&get, &ui::spacer(), &connect], 8.0)), ui::v(&*status)],
        10.0,
    );
    let module = kit::card_padded(&form, 12.0, kit::space::CARD_H);

    let import =
        kit::glass_button("Import ngrok.yml…", Some("square.and.arrow.down"), false, h, || windows::accounts::import_ngrok_yml(None));
    let quit = kit::glass_button("Quit", None, false, h, || app().quit());
    let footer = ui::hstack(&[&import, &ui::spacer(), &quit], 8.0);
    column(&[header("Welcome", None), ui::v(&*module), ui::v(&*footer)], 14.0)
}

// ---------------------------------------------------------------------------
// Main view
// ---------------------------------------------------------------------------

fn main_view(a: &Rc<App>, h: &Handlers) -> Retained<NSStackView> {
    let cfg = a.config.borrow().clone();
    let snaps = a.core.snapshots();
    let online = snaps.iter().filter(|s| s.state.is_online()).count();
    let mut views: Vec<Retained<NSView>> = Vec::new();

    // Header with account selector.
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
    account_popup.setControlSize(objc2_app_kit::NSControlSize::Small);
    account_popup.setToolTip(Some(&ns("Account for quick tunnels and resources")));
    let subtitle = match online {
        0 if snaps.is_empty() => "No active tunnels".to_string(),
        0 => "Connecting…".to_string(),
        1 => "1 tunnel online".to_string(),
        n => format!("{n} tunnels online"),
    };
    views.push(header(&subtitle, (cfg.accounts.len() > 1).then_some(&**account_popup)));

    // Quick tunnel module.
    views.push(quick_module(a, h));

    // Active tunnels.
    if !snaps.is_empty() {
        views.push(section_title("Active"));
        for s in &snaps {
            views.push(tunnel_module(s, h));
        }
    }

    // Profiles module.
    views.push(section_title("Profiles"));
    views.push(profiles_module(a, &cfg, &snaps, h));

    // Footer.
    let open = kit::plain_icon("macwindow", "Open Shaft", 15.0, h, || windows::main::show(Section::Tunnels));
    let inspector = kit::plain_icon("waveform.path.ecg.rectangle", "Inspector", 15.0, h, || windows::main::show(Section::Inspector));
    let resources = kit::plain_icon("globe", "ngrok Resources", 15.0, h, || windows::main::show(Section::Resources));
    let settings = kit::plain_icon("gearshape", "Settings", 15.0, h, || windows::main::show(Section::Settings));
    let quit = kit::glass_button("Quit", None, false, h, || app().quit());
    quit.setControlSize(objc2_app_kit::NSControlSize::Small);
    let footer = ui::hstack(&[&open, &inspector, &resources, &settings, &ui::spacer(), &quit], 16.0);
    ui::pad_edges(&footer, 2.0, 8.0, 0.0, 2.0);
    views.push(ui::v(&*footer));

    column(&views, 12.0)
}

fn quick_module(a: &Rc<App>, h: &Handlers) -> Retained<NSView> {
    let q = a.quick.borrow().clone();
    let labels: Vec<Retained<objc2_foundation::NSString>> = Protocol::ALL.iter().map(|p| ns(p.label())).collect();
    let seg = unsafe {
        NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
            &NSArray::from_retained_slice(&labels),
            NSSegmentSwitchTracking::SelectOne,
            None,
            None,
            ui::mtm(),
        )
    };
    seg.setSelectedSegment(Protocol::ALL.iter().position(|p| *p == q.protocol).unwrap_or(0) as isize);
    seg.setControlSize(objc2_app_kit::NSControlSize::Small);
    let seg_w = objc2::rc::Weak::from_retained(&seg);
    ui::on_action(&seg, h, move || {
        if let Some(s) = seg_w.load() {
            app().quick.borrow_mut().protocol = Protocol::ALL[s.selectedSegment().clamp(0, 2) as usize];
        }
    });

    let field = ui::text_field(&q.upstream, "Port, host:port or URL");
    kit::rounded(&field);
    let field_c = field.clone();
    ui::on_action(&field, h, move || {
        app().quick.borrow_mut().upstream = ui::value(&field_c);
        app().start_quick();
    });
    *a.quick_field.borrow_mut() = Some(field.clone());
    let start = kit::round_button("play.fill", "Start tunnel", true, h, || app().start_quick());

    let title = ui::label("Quick Tunnel");
    title.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, weight::SEMIBOLD)));
    let top = ui::hstack(&[&title, &ui::spacer(), &seg], 8.0);
    let input = ui::fill_hstack(&[&field, &start], 8.0);
    let mut rows = vec![ui::v(&*top), ui::v(&*input)];
    if let Some((name, e)) = a.quick_error.borrow().as_ref() {
        let mut e = e.clone();
        e.message = format!("{name}: {}", e.message);
        rows.push(error_label(&e));
    }
    ui::v(&*kit::card_padded(&column(&rows, 10.0), 12.0, kit::space::CARD_H))
}

fn state_color(s: &TunnelState) -> Retained<NSColor> {
    match s {
        TunnelState::Online => NSColor::systemGreenColor(),
        TunnelState::Starting => NSColor::systemYellowColor(),
        TunnelState::Reconnecting { .. } => NSColor::systemOrangeColor(),
        TunnelState::Failed(_) => NSColor::systemRedColor(),
        TunnelState::Stopping => NSColor::tertiaryLabelColor(),
    }
}

fn tunnel_module(s: &TunnelSnapshot, h: &Handlers) -> Retained<NSView> {
    let run = s.run_id;
    let dot = kit::dot(&state_color(&s.state));
    let name = ui::label(&s.name);
    name.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, weight::SEMIBOLD)));
    ui::resist_compression(&name);
    let proto = kit::badge(s.protocol.label(), &NSColor::systemIndigoColor());

    let mut menu = Vec::new();
    if let Some(url) = &s.public_url {
        let (u1, u2) = (url.clone(), url.clone());
        menu.push(MenuEntry::item("Copy URL", "doc.on.doc", move || app().copy_url(&u1)));
        menu.push(MenuEntry::item("Open in Browser", "safari", move || ui::open_url(&u2)));
    }
    if s.inspect {
        menu.push(MenuEntry::item("Inspect Traffic", "waveform.path.ecg.rectangle", move || windows::inspector::open(Some(run))));
    }
    menu.push(MenuEntry::Separator);
    menu.push(MenuEntry::destructive("Stop", "stop.fill", move || app().core.stop(run)));
    let more = kit::menu_button("More", h, menu);
    let toggle = kit::switch_with(!matches!(s.state, TunnelState::Stopping), h, move |on| {
        if !on {
            app().core.stop(run);
        }
    });
    let top = ui::hstack(&[&dot, &name, &proto, &ui::spacer(), &more, &toggle], 6.0);
    let mut rows = vec![ui::v(&*top)];

    match &s.public_url {
        Some(url) => {
            let u = url.clone();
            let link = ui::link_button(url.trim_start_matches("https://"), h, move || app().copy_url(&u));
            link.setToolTip(Some(&ns("Click to copy")));
            let u2 = url.clone();
            let copy = kit::plain_icon("doc.on.doc", "Copy URL", 12.0, h, move || app().copy_url(&u2));
            rows.push(ui::v(&*ui::hstack(&[&link, &copy, &ui::spacer()], 4.0)));
        }
        None => rows.push(ui::v(&*ui::secondary_label(s.state.label()))),
    }

    let mut meta = format!("→ {}", s.upstream);
    if s.inspect {
        meta.push_str(&format!("  ·  {} requests", s.requests));
    }
    if let TunnelState::Reconnecting { attempt } = s.state {
        meta.push_str(&format!("  ·  reconnecting (#{attempt})"));
    }
    rows.push(ui::v(&*ui::secondary_label(&meta)));
    if s.upstream_reachable == Some(false) {
        let icon = kit::symbol_view("exclamationmark.triangle.fill", 11.0, Some(&NSColor::systemOrangeColor()));
        let l = ui::colored_label("Local service not reachable", &NSColor::systemOrangeColor());
        rows.push(ui::v(&*ui::hstack(&[&icon, &l], 6.0)));
    }
    if let TunnelState::Failed(e) = &s.state {
        rows.push(error_label(e));
    }
    ui::v(&*kit::card_padded(&column(&rows, 6.0), 12.0, kit::space::CARD_H))
}

fn profiles_module(a: &Rc<App>, cfg: &shaft_config::AppConfig, snaps: &[TunnelSnapshot], h: &Handlers) -> Retained<NSView> {
    let mut profiles = cfg.profiles.clone();
    profiles.sort_by_key(|p| (!p.favorite, p.name.to_lowercase()));
    let mut rows: Vec<Retained<NSView>> = Vec::new();
    for p in &profiles {
        let pid = p.id;
        let running = snaps.iter().any(|s| s.profile_id == pid);
        let star = kit::plain_icon(if p.favorite { "star.fill" } else { "star" }, "Favorite", 12.0, h, move || {
            let app = app();
            if let Some(p) = app.config.borrow_mut().profile_mut(pid) {
                p.favorite = !p.favorite;
            }
            app.save();
        });
        if p.favorite {
            star.setContentTintColor(Some(&NSColor::systemYellowColor()));
        }
        let name = ui::label(&p.name);
        ui::resist_compression(&name);
        let acc = cfg.account(p.account_id).map(|a| a.name.as_str()).unwrap_or("?");
        let mut sub = format!("{} → {}", p.protocol().as_str(), p.upstream);
        if cfg.accounts.len() > 1 {
            sub.push_str(&format!(" · {acc}"));
        }
        let sub = ui::secondary_label(&sub);
        let texts = ui::vstack(&[&name, &sub], 1.0);
        let more = kit::menu_button(
            "Profile actions",
            h,
            vec![
                MenuEntry::item("Edit…", "slider.horizontal.3", move || windows::profile::open(Some(pid))),
                MenuEntry::item(if running { "Stop" } else { "Start" }, if running { "stop.fill" } else { "play.fill" }, move || {
                    app().toggle_profile(pid)
                }),
            ],
        );
        let toggle = kit::play_button(running, h, move || app().toggle_profile(pid));
        let row = ui::hstack(&[&star, &texts, &ui::spacer(), &more, &toggle], 10.0);
        let mut block = vec![ui::v(&*row)];
        if !running {
            if let Some(e) = a.failures.borrow().get(&pid) {
                block.push(error_label(e));
            }
        }
        rows.push(ui::v(&*column(&block, 6.0)));
    }
    if rows.is_empty() {
        rows.push(ui::v(&*kit::footnote("Save tunnels you use often as profiles.", WIDTH - 60.0)));
    }
    let new = ui::link_button("New Profile…", h, || windows::profile::open(None));
    new.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    let stop_all = ui::link_button("Stop All", h, || app().core.stop_all());
    stop_all.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    stop_all.setHidden(snaps.is_empty());
    rows.push(ui::v(&*ui::hstack(&[&new, &ui::spacer(), &stop_all], 8.0)));
    ui::v(&*kit::group(&rows))
}
