//! The Shaft window ([`main`]) and its section panes. Each pane controller is
//! built lazily the first time its section is shown and kept in [`Windows`].

pub mod accounts;
pub mod diagnostics;
pub mod inspector;
pub mod main;
pub mod profile;
pub mod resources;
pub mod settings;

use std::{cell::RefCell, rc::Rc};

use main::Section;
use uuid::Uuid;

use crate::app::app;

#[derive(Default)]
pub struct Windows {
    pub profile: RefCell<Option<Rc<profile::TunnelsPane>>>,
    pub accounts: RefCell<Option<Rc<accounts::AccountsPane>>>,
    pub inspector: RefCell<Option<Rc<inspector::InspectorPane>>>,
    pub resources: RefCell<Option<Rc<resources::ResourcesPane>>>,
    pub settings: RefCell<Option<Rc<settings::SettingsPane>>>,
    pub diagnostics: RefCell<Option<Rc<diagnostics::DiagnosticsPane>>>,
}

impl Windows {
    pub fn inspector_open(&self) -> bool {
        main::is_showing(Section::Inspector)
    }

    pub fn inspector_changed(&self, run_id: Uuid) {
        let w = self.inspector.borrow().clone();
        if let (Some(w), true) = (w, self.inspector_open()) {
            w.traffic_changed(run_id);
        }
    }

    pub fn inspector_runs_changed(&self) {
        let w = self.inspector.borrow().clone();
        if let (Some(w), true) = (w, self.inspector_open()) {
            w.reload_runs();
        }
    }

    /// Refresh panes that show account/profile/tunnel state.
    pub fn refresh_profiles_dependent(&self) {
        if main::is_showing(Section::Accounts) {
            let w = self.accounts.borrow().clone();
            if let Some(w) = w {
                w.reload_list();
            }
        }
        if main::is_showing(Section::Tunnels) {
            let w = self.profile.borrow().clone();
            if let Some(w) = w {
                w.reload_list();
            }
        }
    }
}

/// Called whenever a section becomes visible, to refresh its data.
pub fn pane_shown(section: Section) {
    let w = &app().windows;
    match section {
        Section::Tunnels => {
            let p = w.profile.borrow().clone();
            if let Some(p) = p {
                p.reload_list();
            }
        }
        Section::Inspector => {
            let p = w.inspector.borrow().clone();
            if let Some(p) = p {
                p.reload_runs();
            }
        }
        Section::Resources => {
            let p = w.resources.borrow().clone();
            if let Some(p) = p {
                p.shown();
            }
        }
        Section::Accounts => {
            let p = w.accounts.borrow().clone();
            if let Some(p) = p {
                p.reload_list();
            }
        }
        Section::Settings | Section::Diagnostics => {}
    }
}

/// Split "a, b\nc" into trimmed, non-empty items.
pub fn split_list(s: &str) -> Vec<String> {
    s.split([',', '\n']).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

pub fn join_list(v: &[String]) -> String {
    v.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists() {
        assert_eq!(split_list(" a, b\n\nc ,"), vec!["a", "b", "c"]);
        assert_eq!(join_list(&["a".into(), "b".into()]), "a, b");
    }
}
