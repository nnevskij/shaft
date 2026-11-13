//! Settings pane (System Settings style).

use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::{NSPopUpButton, NSSwitch, NSTextField, NSView};
use shaft_config::{app_support_dir, log_dir, MIN_HEARTBEAT_SECS};

use super::main::Section;
use crate::{
    app::app,
    kit,
    ui::{self, ns, Handlers},
    updater,
};

const LOG_LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];
const LW: f64 = 190.0;

pub struct SettingsPane {
    pub view: Retained<NSView>,
    _h: Handlers,
    copy_url: Retained<NSSwitch>,
    n_online: Retained<NSSwitch>,
    n_down: Retained<NSSwitch>,
    n_reconnected: Retained<NSSwitch>,
    n_errors: Retained<NSSwitch>,
    n_copied: Retained<NSSwitch>,
    hk_popover: Retained<NSTextField>,
    hk_copy: Retained<NSTextField>,
    max_requests: Retained<NSTextField>,
    max_body_kb: Retained<NSTextField>,
    clear_on_stop: Retained<NSSwitch>,
    max_attempts: Retained<NSTextField>,
    heartbeat: Retained<NSTextField>,
    log_level: Retained<NSPopUpButton>,
    auto_update: Retained<NSSwitch>,
    update_status: Retained<NSTextField>,
    check_now: Retained<objc2_app_kit::NSButton>,
    status: Retained<NSTextField>,
}

pub fn open() {
    super::main::show(Section::Settings);
}

pub fn pane() -> Retained<NSView> {
    let p = Rc::new(build());
    *app().windows.settings.borrow_mut() = Some(p.clone());
    p.view.clone()
}

fn with(f: impl FnOnce(&SettingsPane)) {
    let w = app().windows.settings.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

/// Show the updater's latest state, if the pane exists.
pub fn refresh_updates() {
    with(|w| {
        w.update_status.setStringValue(&ns(&updater::status()));
        w.check_now.setEnabled(!updater::is_checking());
    });
}

fn version_string() -> String {
    let build = objc2_foundation::NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&ns("CFBundleVersion"))
        .and_then(|v| v.downcast::<objc2_foundation::NSString>().ok())
        .map(|s| s.to_string());
    match build {
        Some(b) => format!("Shaft {} ({b})", env!("CARGO_PKG_VERSION")),
        None => format!("Shaft {}", env!("CARGO_PKG_VERSION")),
    }
}

fn number_field(value: impl ToString, placeholder: &str) -> Retained<NSTextField> {
    let f = ui::text_field(&value.to_string(), placeholder);
    f.setAlignment(objc2_app_kit::NSTextAlignment::Right);
    ui::set_width(&f, 90.0);
    f
}

fn build() -> SettingsPane {
    let app = app();
    let s = app.config.borrow().settings.clone();
    let h = Handlers::new();

    let copy_url = kit::switch(s.copy_url_on_start);
    let n_online = kit::switch(s.notifications.tunnel_online);
    let n_down = kit::switch(s.notifications.tunnel_down);
    let n_reconnected = kit::switch(s.notifications.tunnel_reconnected);
    let n_errors = kit::switch(s.notifications.errors);
    let n_copied = kit::switch(s.notifications.url_copied);
    let hk_popover = ui::text_field(&s.hotkeys.toggle_popover, "e.g. cmd+shift+n");
    let hk_copy = ui::text_field(&s.hotkeys.copy_last_url, "e.g. cmd+shift+c");
    let max_requests = number_field(s.inspector.max_requests, "500");
    let max_body_kb = number_field(s.inspector.max_body_bytes / 1024, "1024");
    let clear_on_stop = kit::switch(s.inspector.clear_on_stop);
    let max_attempts = number_field(s.reconnect.max_attempts, "10");
    let heartbeat = number_field(s.reconnect.heartbeat().0, "10");
    let log_level = ui::popup(&LOG_LEVELS, LOG_LEVELS.iter().position(|l| *l == s.log_level).unwrap_or(2));
    let auto_update = kit::switch(s.updates.check_automatically);

    let general = kit::section(
        Some("General"),
        &[kit::row_detail("Copy URL when online", "Put a tunnel's public URL on the clipboard as soon as it starts", &copy_url)],
        None,
    );
    let notifications = kit::section(
        Some("Notifications"),
        &[
            kit::row("Tunnel online", &n_online),
            kit::row("Connection lost", &n_down),
            kit::row("Reconnected", &n_reconnected),
            kit::row_detail("Errors", "Authentication, plan limits, failed tunnels", &n_errors),
            kit::row("URL copied", &n_copied),
        ],
        None,
    );
    let shortcuts = kit::section(
        Some("Keyboard Shortcuts"),
        &[kit::row_fill("Show Shaft menu", &hk_popover, LW), kit::row_fill("Copy last URL", &hk_copy, LW)],
        Some("Global shortcuts work from any app. Modifiers: cmd, shift, alt/option, ctrl. Per-profile shortcuts are set in each profile."),
    );
    let inspector = kit::section(
        Some("Inspector"),
        &[
            kit::row("Requests kept per tunnel", &max_requests),
            kit::row("Body capture limit (KB)", &max_body_kb),
            kit::row_detail("Discard traffic on stop", "Forget captured requests when a tunnel stops", &clear_on_stop),
        ],
        None,
    );
    let connection = kit::section(
        Some("Connection"),
        &[
            kit::row("Reconnect attempts", &max_attempts),
            kit::row_detail("Heartbeat (seconds)", &format!("Minimum {MIN_HEARTBEAT_SECS}; lower detects drops faster"), &heartbeat),
        ],
        None,
    );
    let check_now = kit::glass_button("Check Now", Some("arrow.clockwise"), false, &h, || updater::check(updater::Trigger::Manual));
    check_now.setEnabled(!updater::is_checking());
    let version = ui::label(&version_string());
    let update_status = ui::secondary_label(&updater::status());
    let version_texts = ui::vstack(&[&version, &update_status], 2.0);
    let updates = kit::section(
        Some("Software Update"),
        &[
            kit::row_detail("Check for updates automatically", "At launch and once a day, from GitHub releases", &auto_update),
            ui::v(&*ui::hstack(&[&version_texts, &ui::spacer(), &check_now], 8.0)),
        ],
        Some("Shaft only tells you about new versions; it never installs anything on its own."),
    );
    let open_logs = kit::glass_button("Show Logs", Some("doc.text.magnifyingglass"), false, &h, || ui::reveal_in_finder(&log_dir()));
    let open_config =
        kit::glass_button("Show Config", Some("folder"), false, &h, || ui::reveal_in_finder(&app_support_dir().join("config.toml")));
    let troubleshooting = kit::section(
        Some("Troubleshooting"),
        &[kit::row("Log level", &log_level), ui::v(&*ui::hstack(&[&open_logs, &open_config, &ui::spacer()], 8.0))],
        None,
    );

    let problems = app.hotkey_problems.borrow().join("\n");
    let status = kit::footnote(&problems, 560.0);
    let save = kit::glass_button("Save", None, true, &h, || with(|w| w.save()));
    save.setKeyEquivalent(&ns("s"));
    save.setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags::Command);
    let buttons = ui::hstack(&[&status, &ui::spacer(), &save], 8.0);

    let content = ui::vstack(&[&general, &notifications, &shortcuts, &inspector, &connection, &updates, &troubleshooting, &buttons], 28.0);
    for v in [&*general as &NSView, &notifications, &shortcuts, &inspector, &connection, &updates, &troubleshooting, &buttons] {
        ui::fill_width(&content, v);
    }
    ui::pad(&content, 20.0, 28.0);
    // Keep forms at a readable width, centered in wide windows.
    let column = NSView::new(ui::mtm());
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    column.addSubview(&content);
    content.topAnchor().constraintEqualToAnchor(&column.topAnchor()).setActive(true);
    content.bottomAnchor().constraintEqualToAnchor(&column.bottomAnchor()).setActive(true);
    content.centerXAnchor().constraintEqualToAnchor(&column.centerXAnchor()).setActive(true);
    content.widthAnchor().constraintLessThanOrEqualToConstant(680.0).setActive(true);
    let w = content.widthAnchor().constraintEqualToAnchor(&column.widthAnchor());
    w.setPriority(500.0);
    w.setActive(true);
    let col_stack = ui::vstack(&[&column], 0.0);
    ui::fill_width(&col_stack, &column);
    let scroll = ui::scroll_stack(&col_stack);

    SettingsPane {
        view: ui::v(&*scroll),
        _h: h,
        copy_url,
        n_online,
        n_down,
        n_reconnected,
        n_errors,
        n_copied,
        hk_popover,
        hk_copy,
        max_requests,
        max_body_kb,
        clear_on_stop,
        max_attempts,
        heartbeat,
        log_level,
        auto_update,
        update_status,
        check_now,
        status,
    }
}

impl SettingsPane {
    fn save(&self) {
        let num = |f: &NSTextField, name: &str, min: u64, max: u64| -> Result<u64, String> {
            match ui::value(f).parse::<u64>() {
                Ok(v) if (min..=max).contains(&v) => Ok(v),
                _ => Err(format!("{name} must be a number between {min} and {max}")),
            }
        };
        let parsed = (|| -> Result<_, String> {
            Ok((
                num(&self.max_requests, "Requests kept", 1, 100_000)?,
                num(&self.max_body_kb, "Body capture limit", 0, 100 * 1024)?,
                num(&self.max_attempts, "Reconnect attempts", 1, 1000)?,
                num(&self.heartbeat, "Heartbeat", MIN_HEARTBEAT_SECS, 300)?,
            ))
        })();
        let (max_req, body_kb, attempts, hb) = match parsed {
            Ok(v) => v,
            Err(e) => return self.status.setStringValue(&ns(&e)),
        };
        for f in [&self.hk_popover, &self.hk_copy] {
            let v = ui::value(f);
            if !v.is_empty() {
                if let Err(e) = crate::platform::parse_accelerator(&v) {
                    return self.status.setStringValue(&ns(&format!("Shortcut `{v}`: {e}")));
                }
            }
        }
        let app = app();
        let old_level = app.config.borrow().settings.log_level.clone();
        {
            let mut cfg = app.config.borrow_mut();
            let s = &mut cfg.settings;
            s.copy_url_on_start = kit::is_on(&self.copy_url);
            s.notifications.tunnel_online = kit::is_on(&self.n_online);
            s.notifications.tunnel_down = kit::is_on(&self.n_down);
            s.notifications.tunnel_reconnected = kit::is_on(&self.n_reconnected);
            s.notifications.errors = kit::is_on(&self.n_errors);
            s.notifications.url_copied = kit::is_on(&self.n_copied);
            s.hotkeys.toggle_popover = ui::value(&self.hk_popover);
            s.hotkeys.copy_last_url = ui::value(&self.hk_copy);
            s.inspector.max_requests = max_req as usize;
            s.inspector.max_body_bytes = (body_kb * 1024) as usize;
            s.inspector.clear_on_stop = kit::is_on(&self.clear_on_stop);
            s.reconnect.max_attempts = attempts as u32;
            s.reconnect.heartbeat_interval_secs = hb;
            s.reconnect.heartbeat_tolerance_secs = hb + 5;
            s.updates.check_automatically = kit::is_on(&self.auto_update);
            s.log_level = LOG_LEVELS[ui::selected(&self.log_level).min(LOG_LEVELS.len() - 1)].into();
        }
        if !app.save() {
            return;
        }
        let new_level = app.config.borrow().settings.log_level.clone();
        if new_level != old_level {
            crate::logging::set_level(&new_level);
        }
        let problems = app.hotkey_problems.borrow().clone();
        if problems.is_empty() {
            self.status.setStringValue(&ns("Saved."));
        } else {
            self.status.setStringValue(&ns(&format!("Saved, but some shortcuts could not be registered:\n{}", problems.join("\n"))));
        }
    }
}
