//! Global settings.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField, NSView, NSWindow};
use shaft_config::{app_support_dir, log_dir};

use crate::{
    app::app,
    ui::{self, ns, Handlers},
};

const LOG_LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

pub struct SettingsWindow {
    pub window: Retained<NSWindow>,
    _handlers: Handlers,
    copy_url: Retained<NSButton>,
    n_online: Retained<NSButton>,
    n_down: Retained<NSButton>,
    n_reconnected: Retained<NSButton>,
    n_errors: Retained<NSButton>,
    n_copied: Retained<NSButton>,
    hk_popover: Retained<NSTextField>,
    hk_copy: Retained<NSTextField>,
    max_requests: Retained<NSTextField>,
    max_body_kb: Retained<NSTextField>,
    clear_on_stop: Retained<NSButton>,
    max_attempts: Retained<NSTextField>,
    heartbeat: Retained<NSTextField>,
    log_level: Retained<NSPopUpButton>,
    status: Retained<NSTextField>,
}

pub fn open() {
    let app = app();
    app.close_popover();
    if let Some(old) = app.windows.settings.borrow_mut().take() {
        old.window.close();
    }
    let w = Rc::new(build());
    ui::show_window(&w.window);
    *app.windows.settings.borrow_mut() = Some(w);
}

fn with(f: impl FnOnce(&SettingsWindow)) {
    let w = app().windows.settings.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn build() -> SettingsWindow {
    let app = app();
    let s = app.config.borrow().settings.clone();
    let h = Handlers::new();
    let window = ui::window("Shaft Settings", 560.0, 640.0, false);

    let copy_url = ui::checkbox("Copy the public URL when a tunnel comes online", s.copy_url_on_start);
    let n_online = ui::checkbox("Tunnel online", s.notifications.tunnel_online);
    let n_down = ui::checkbox("Connection lost / tunnel down", s.notifications.tunnel_down);
    let n_reconnected = ui::checkbox("Reconnected", s.notifications.tunnel_reconnected);
    let n_errors = ui::checkbox("Errors (auth, plan limits, …)", s.notifications.errors);
    let n_copied = ui::checkbox("URL copied", s.notifications.url_copied);
    let hk_popover = ui::text_field(&s.hotkeys.toggle_popover, "e.g. cmd+shift+n");
    let hk_copy = ui::text_field(&s.hotkeys.copy_last_url, "e.g. cmd+shift+c");
    let max_requests = ui::text_field(&s.inspector.max_requests.to_string(), "500");
    let max_body_kb = ui::text_field(&(s.inspector.max_body_bytes / 1024).to_string(), "1024");
    let clear_on_stop = ui::checkbox("Discard captured traffic when a tunnel stops", s.inspector.clear_on_stop);
    let max_attempts = ui::text_field(&s.reconnect.max_attempts.to_string(), "10");
    let heartbeat = ui::text_field(&s.reconnect.heartbeat_interval_secs.to_string(), "5");
    let log_level = ui::popup(&LOG_LEVELS, LOG_LEVELS.iter().position(|l| *l == s.log_level).unwrap_or(2));

    let problems = app.hotkey_problems.borrow().join("\n");
    let status = ui::wrapping_label(&problems, 500.0);

    let general = ui::form(&[("", &copy_url)], 160.0);
    let notif = ui::form(&[("Notify on", &n_online), ("", &n_down), ("", &n_reconnected), ("", &n_errors), ("", &n_copied)], 160.0);
    let keys = ui::form(&[("Show/hide menu", &hk_popover), ("Copy last URL", &hk_copy)], 160.0);
    let keys_note = ui::secondary_label("Per-profile shortcuts are set in each profile. Modifiers: cmd, shift, alt/option, ctrl.");
    let inspector =
        ui::form(&[("Requests kept per tunnel", &max_requests), ("Body capture limit (KB)", &max_body_kb), ("", &clear_on_stop)], 160.0);
    let network = ui::form(&[("Reconnect attempts", &max_attempts), ("Heartbeat (seconds)", &heartbeat)], 160.0);
    let logs = ui::form(&[("Log level", &log_level)], 160.0);

    let open_logs = ui::button("Open logs", &h, || ui::reveal_in_finder(&log_dir()));
    let open_config = ui::button("Open config folder", &h, || ui::reveal_in_finder(&app_support_dir().join("config.toml")));
    let save = ui::button("Save", &h, || with(|w| w.save()));
    save.setKeyEquivalent(&ns("\r"));
    let buttons = ui::hstack(&[&open_logs, &open_config, &ui::spacer(), &save], 8.0);

    let root = ui::vstack(
        &[
            &general,
            &ui::separator(),
            &notif,
            &ui::separator(),
            &keys,
            &keys_note,
            &ui::separator(),
            &ui::section_label("Inspector"),
            &inspector,
            &ui::separator(),
            &ui::section_label("Connection"),
            &network,
            &logs,
            &status,
            &buttons,
        ],
        10.0,
    );
    for v in [&*general as &NSView, &notif, &keys, &inspector, &network, &logs, &buttons] {
        ui::fill_width(&root, v);
    }
    ui::set_window_content_top(&window, &root);

    SettingsWindow {
        window,
        _handlers: h,
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
        status,
    }
}

impl SettingsWindow {
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
                num(&self.heartbeat, "Heartbeat", 1, 300)?,
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
            s.copy_url_on_start = ui::is_checked(&self.copy_url);
            s.notifications.tunnel_online = ui::is_checked(&self.n_online);
            s.notifications.tunnel_down = ui::is_checked(&self.n_down);
            s.notifications.tunnel_reconnected = ui::is_checked(&self.n_reconnected);
            s.notifications.errors = ui::is_checked(&self.n_errors);
            s.notifications.url_copied = ui::is_checked(&self.n_copied);
            s.hotkeys.toggle_popover = ui::value(&self.hk_popover);
            s.hotkeys.copy_last_url = ui::value(&self.hk_copy);
            s.inspector.max_requests = max_req as usize;
            s.inspector.max_body_bytes = (body_kb * 1024) as usize;
            s.inspector.clear_on_stop = ui::is_checked(&self.clear_on_stop);
            s.reconnect.max_attempts = attempts as u32;
            s.reconnect.heartbeat_interval_secs = hb;
            s.reconnect.heartbeat_tolerance_secs = (hb * 2).max(hb + 1);
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
            self.status.setStringValue(&ns("Saved. Connection settings apply to newly started sessions."));
        } else {
            self.status.setStringValue(&ns(&format!("Saved, but some shortcuts could not be registered:\n{}", problems.join("\n"))));
        }
    }
}
