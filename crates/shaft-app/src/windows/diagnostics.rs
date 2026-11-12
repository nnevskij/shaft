//! Diagnostics window (`ngrok diagnose` / `ngrok version` parity).

use std::{cell::RefCell, rc::Rc};

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextView, NSView, NSWindow};
use uuid::Uuid;

use crate::{
    app::app,
    platform::on_main,
    ui::{self, ns, Handlers},
};

pub struct DiagnosticsWindow {
    pub window: Retained<NSWindow>,
    _handlers: Handlers,
    account_ids: Vec<Uuid>,
    account: Retained<NSPopUpButton>,
    output: Retained<NSTextView>,
    run: Retained<NSButton>,
    report: RefCell<String>,
}

pub fn open() {
    let app = app();
    app.close_popover();
    if let Some(old) = app.windows.diagnostics.borrow_mut().take() {
        old.window.close();
    }
    let w = Rc::new(build());
    ui::show_window(&w.window);
    *app.windows.diagnostics.borrow_mut() = Some(w.clone());
    w.start();
}

fn with(f: impl FnOnce(&DiagnosticsWindow)) {
    let w = app().windows.diagnostics.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn build() -> DiagnosticsWindow {
    let app = app();
    let cfg = app.config.borrow().clone();
    let h = Handlers::new();
    let window = ui::window("Diagnostics", 620.0, 420.0, true);
    let account_ids: Vec<Uuid> = cfg.accounts.iter().map(|a| a.id).collect();
    let names: Vec<&str> = cfg.accounts.iter().map(|a| a.name.as_str()).collect();
    let sel = cfg.active_account().and_then(|a| account_ids.iter().position(|i| *i == a.id)).unwrap_or(0);
    let account = if names.is_empty() { ui::popup(&["(no account)"], 0) } else { ui::popup(&names, sel) };
    let run = ui::button("Run checks", &h, || with(|w| w.start()));
    let copy = ui::button("Copy report", &h, || with(|w| ui::copy_to_clipboard(&w.report.borrow())));
    let bar = ui::fill_hstack(&[&ui::label("Account"), &account, &ui::spacer(), &copy, &run], 8.0);
    let (scroll, output) = ui::text_area("", true, false);
    ui::set_min_height(&scroll, 300.0);
    let note = ui::secondary_label(
        "Checks DNS, TCP and TLS to the ngrok edge, then validates the authtoken and API key. No secrets are included in the report.",
    );
    let root = ui::vstack(&[&bar, &scroll, &note], 10.0);
    for v in [&*bar as &NSView, &scroll] {
        ui::fill_width(&root, v);
    }
    ui::set_window_content(&window, &root);
    DiagnosticsWindow { window, _handlers: h, account_ids, account, output, run, report: RefCell::new(String::new()) }
}

impl DiagnosticsWindow {
    fn start(&self) {
        let app = app();
        let account = self.account_ids.get(ui::selected(&self.account)).and_then(|id| app.config.borrow().account(*id).cloned());
        self.run.setEnabled(false);
        self.output.setString(&ns("Running…"));
        app.core.runtime().spawn(async move {
            let checks = shaft_core::diagnostics::run(account.as_ref()).await;
            let report = shaft_core::diagnostics::report(&checks);
            on_main(move || {
                with(|w| {
                    w.output.setString(&ns(&report));
                    *w.report.borrow_mut() = report;
                    w.run.setEnabled(true);
                })
            });
        });
    }
}
