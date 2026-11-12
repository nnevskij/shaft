//! On-demand windows. Each window controller is kept alive in [`Windows`]
//! while open and rebuilt when reopened.

pub mod accounts;
pub mod diagnostics;
pub mod inspector;
pub mod profile;
pub mod resources;
pub mod settings;

use std::{cell::RefCell, rc::Rc};

use uuid::Uuid;

#[derive(Default)]
pub struct Windows {
    pub profile: RefCell<Option<Rc<profile::ProfileWindow>>>,
    pub accounts: RefCell<Option<Rc<accounts::AccountsWindow>>>,
    pub inspector: RefCell<Option<Rc<inspector::InspectorWindow>>>,
    pub resources: RefCell<Option<Rc<resources::ResourcesWindow>>>,
    pub settings: RefCell<Option<Rc<settings::SettingsWindow>>>,
    pub diagnostics: RefCell<Option<Rc<diagnostics::DiagnosticsWindow>>>,
}

impl Windows {
    pub fn inspector_open(&self) -> bool {
        self.inspector.borrow().as_ref().map(|w| w.window.isVisible()).unwrap_or(false)
    }

    pub fn inspector_changed(&self, run_id: Uuid) {
        let w = self.inspector.borrow().clone();
        if let Some(w) = w {
            if w.window.isVisible() {
                w.traffic_changed(run_id);
            }
        }
    }

    pub fn inspector_runs_changed(&self) {
        let w = self.inspector.borrow().clone();
        if let Some(w) = w {
            if w.window.isVisible() {
                w.reload_runs();
            }
        }
    }

    /// Refresh windows that show the account/profile lists.
    pub fn refresh_profiles_dependent(&self) {
        let w = self.accounts.borrow().clone();
        if let Some(w) = w {
            if w.window.isVisible() {
                w.reload_list();
            }
        }
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
