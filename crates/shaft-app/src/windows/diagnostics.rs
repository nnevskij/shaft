//! Diagnostics pane (`ngrok diagnose` / `ngrok version` parity).

use std::{cell::RefCell, rc::Rc};

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSColor, NSPopUpButton, NSStackView, NSView};
use shaft_core::diagnostics::{Check, CheckStatus};
use uuid::Uuid;

use super::main::Section;
use crate::{
    app::app,
    kit,
    platform::on_main,
    ui::{self, ns, Handlers},
};

pub struct DiagnosticsPane {
    pub view: Retained<NSView>,
    _h: Handlers,
    account_ids: Vec<Uuid>,
    account: Retained<NSPopUpButton>,
    results: Retained<NSStackView>,
    run: Retained<NSButton>,
    report: RefCell<String>,
}

pub fn open() {
    super::main::show(Section::Diagnostics);
}

pub fn pane() -> Retained<NSView> {
    let p = Rc::new(build());
    *app().windows.diagnostics.borrow_mut() = Some(p.clone());
    p.start();
    p.view.clone()
}

fn with(f: impl FnOnce(&DiagnosticsPane)) {
    let w = app().windows.diagnostics.borrow().clone();
    if let Some(w) = w {
        f(&w);
    }
}

fn build() -> DiagnosticsPane {
    let cfg = app().config.borrow().clone();
    let h = Handlers::new();
    let account_ids: Vec<Uuid> = cfg.accounts.iter().map(|a| a.id).collect();
    let names: Vec<&str> = cfg.accounts.iter().map(|a| a.name.as_str()).collect();
    let sel = cfg.active_account().and_then(|a| account_ids.iter().position(|i| *i == a.id)).unwrap_or(0);
    let account = if names.is_empty() { ui::popup(&["No account"], 0) } else { ui::popup(&names, sel) };
    let run = kit::glass_button("Run Again", Some("arrow.clockwise"), true, &h, || with(|w| w.start()));
    let copy = kit::glass_button("Copy Report", Some("doc.on.doc"), false, &h, || with(|w| ui::copy_to_clipboard(&w.report.borrow())));
    let setup = kit::section(Some("Check"), &[kit::row("Account", &account)], None);
    let results = ui::vstack(&[], 0.0);
    let note = kit::footnote(
        "Checks DNS, TCP and TLS to the ngrok edge, then validates the authtoken and the API key. The report contains no secrets.",
        560.0,
    );
    let buttons = ui::hstack(&[&copy, &ui::spacer(), &run], 8.0);
    let content = ui::vstack(&[&setup, &results, &note, &buttons], 28.0);
    for v in [&*setup as &NSView, &results, &note, &buttons] {
        ui::fill_width(&content, v);
    }
    ui::pad(&content, 20.0, 28.0);
    let scroll = ui::scroll_stack(&content);
    DiagnosticsPane { view: ui::v(&*scroll), _h: h, account_ids, account, results, run, report: RefCell::new(String::new()) }
}

fn result_row(c: &Check) -> Retained<NSView> {
    let (symbol, color) = match c.status {
        CheckStatus::Ok => ("checkmark.circle.fill", NSColor::systemGreenColor()),
        CheckStatus::Warning => ("exclamationmark.triangle.fill", NSColor::systemOrangeColor()),
        CheckStatus::Failed => ("xmark.octagon.fill", NSColor::systemRedColor()),
        CheckStatus::Skipped => ("minus.circle", NSColor::tertiaryLabelColor()),
    };
    let icon = kit::symbol_view(symbol, 16.0, Some(&color));
    let name = ui::label(&c.name);
    ui::resist_compression(&name);
    let detail = ui::wrapping_label(&c.detail, 380.0);
    detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
    detail.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(11.0)));
    let texts = ui::vstack(&[&name, &detail], 2.0);
    let time = ui::secondary_label(&c.duration.map(|d| format!("{} ms", d.as_millis())).unwrap_or_default());
    let row = ui::hstack(&[&icon, &texts, &ui::spacer(), &time], 10.0);
    row.setAlignment(objc2_app_kit::NSLayoutAttribute::Top);
    ui::v(&*row)
}

impl DiagnosticsPane {
    fn set_results(&self, rows: Vec<Retained<NSView>>) {
        for v in self.results.arrangedSubviews().iter() {
            v.removeFromSuperview();
        }
        let section = kit::section(Some("Results"), &rows, None);
        self.results.addArrangedSubview(&section);
        ui::fill_width(&self.results, &section);
    }

    fn start(&self) {
        let app = app();
        let account = self.account_ids.get(ui::selected(&self.account)).and_then(|id| app.config.borrow().account(*id).cloned());
        self.run.setEnabled(false);
        let spinner = ui::secondary_label("Running checks…");
        self.set_results(vec![ui::v(&*spinner)]);
        app.core.runtime().spawn(async move {
            let checks = shaft_core::diagnostics::run(account.as_ref()).await;
            on_main(move || {
                with(|w| {
                    *w.report.borrow_mut() = shaft_core::diagnostics::report(&checks);
                    w.set_results(checks.iter().map(result_row).collect());
                    w.run.setEnabled(true);
                    let _ = ns("");
                })
            });
        });
    }
}
