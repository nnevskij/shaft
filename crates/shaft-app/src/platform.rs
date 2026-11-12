//! macOS integration: main-thread dispatch, notifications, global hotkeys,
//! single-instance lock and the (hidden) main menu.

use std::{collections::HashMap, fs::File, os::fd::AsRawFd, path::Path, str::FromStr};

use block2::RcBlock;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::{rc::Retained, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{NSBundle, NSDistributedNotificationCenter, NSNotification, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNNotificationSound, UNUserNotificationCenter,
};
use uuid::Uuid;

use crate::ui::{mtm, ns};

/// Run `f` on the main thread (asynchronously).
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(f);
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

pub struct Notifier {
    center: Option<Retained<UNUserNotificationCenter>>,
    authorized_requested: std::cell::Cell<bool>,
}

impl Notifier {
    /// UserNotifications only works from a signed .app bundle; when running
    /// the bare binary (e.g. `cargo run`) notifications are logged instead.
    pub fn new() -> Self {
        let bundled = NSBundle::mainBundle().bundleIdentifier().is_some();
        let center = bundled.then(UNUserNotificationCenter::currentNotificationCenter);
        if center.is_none() {
            tracing::info!("not running from an app bundle: notifications will be logged only");
        }
        Self { center, authorized_requested: std::cell::Cell::new(false) }
    }

    fn ensure_authorized(&self) {
        let Some(c) = &self.center else { return };
        if self.authorized_requested.replace(true) {
            return;
        }
        let block = RcBlock::new(|granted: objc2::runtime::Bool, _err: *mut objc2_foundation::NSError| {
            tracing::info!(granted = granted.as_bool(), "notification authorization");
        });
        c.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &block);
    }

    pub fn notify(&self, title: &str, body: &str) {
        tracing::info!(title, body, "notification");
        let Some(c) = &self.center else { return };
        // Ask for permission on first use rather than at launch.
        self.ensure_authorized();
        let content = UNMutableNotificationContent::new();
        content.setTitle(&ns(title));
        content.setBody(&ns(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&ns(&Uuid::new_v4().to_string()), &content, None);
        c.addNotificationRequest_withCompletionHandler(&req, None);
    }
}

// ---------------------------------------------------------------------------
// Global hotkeys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    TogglePopover,
    CopyLastUrl,
    ToggleProfile(Uuid),
}

pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    actions: HashMap<u32, HotkeyAction>,
}

impl Hotkeys {
    pub fn new() -> Option<Self> {
        match GlobalHotKeyManager::new() {
            Ok(manager) => {
                GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
                    if e.state() == HotKeyState::Pressed {
                        let id = e.id();
                        on_main(move || crate::app::hotkey_pressed(id));
                    }
                }));
                Some(Self { manager, registered: Vec::new(), actions: HashMap::new() })
            }
            Err(e) => {
                tracing::warn!(error = %e, "global hotkeys unavailable");
                None
            }
        }
    }

    /// Replace all registrations. Returns human-readable problems.
    pub fn set(&mut self, bindings: &[(String, HotkeyAction)]) -> Vec<String> {
        for hk in self.registered.drain(..) {
            let _ = self.manager.unregister(hk);
        }
        self.actions.clear();
        let mut problems = Vec::new();
        for (accel, action) in bindings {
            if accel.trim().is_empty() {
                continue;
            }
            match parse_accelerator(accel) {
                Ok(hk) => match self.manager.register(hk) {
                    Ok(()) => {
                        self.actions.insert(hk.id(), *action);
                        self.registered.push(hk);
                    }
                    Err(e) => problems.push(format!("`{accel}`: {e}")),
                },
                Err(e) => problems.push(format!("`{accel}`: {e}")),
            }
        }
        problems
    }

    pub fn action(&self, id: u32) -> Option<HotkeyAction> {
        self.actions.get(&id).copied()
    }
}

/// Accept friendly forms like `cmd+shift+n` as well as `cmd+shift+KeyN`.
pub fn parse_accelerator(s: &str) -> Result<HotKey, String> {
    let parts: Vec<String> = s
        .split('+')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| match p.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "⌘" => "super".to_string(),
            "option" | "opt" | "⌥" => "alt".to_string(),
            "control" | "⌃" => "ctrl".to_string(),
            "⇧" => "shift".to_string(),
            k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphabetic()) => format!("Key{}", k.to_ascii_uppercase()),
            k if k.len() == 1 && k.chars().all(|c| c.is_ascii_digit()) => format!("Digit{k}"),
            _ => p.to_string(),
        })
        .collect();
    if parts.len() < 2 {
        return Err("needs at least one modifier and a key".into());
    }
    HotKey::from_str(&parts.join("+")).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Single instance
// ---------------------------------------------------------------------------

const ACTIVATE_NOTIFICATION: &str = "dev.shaft.app.activate";

/// Holds an exclusive lock for the lifetime of the process.
pub struct InstanceLock {
    _file: File,
}

/// Take the lock, or tell the running instance to show itself and return None.
pub fn acquire_instance_lock(dir: &Path) -> Option<InstanceLock> {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("instance.lock");
    let file = match File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "cannot create instance lock; continuing without it");
            return Some(InstanceLock { _file: File::open("/dev/null").expect("/dev/null") });
        }
    };
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Some(InstanceLock { _file: file })
    } else {
        let center = NSDistributedNotificationCenter::defaultCenter();
        unsafe {
            center.postNotificationName_object_userInfo_deliverImmediately(&ns(ACTIVATE_NOTIFICATION), None, None, true);
        }
        None
    }
}

/// Show the popover when another launch asks us to.
pub fn observe_activation_requests() {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let block = RcBlock::new(|_n: std::ptr::NonNull<NSNotification>| {
        crate::app::show_popover();
    });
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(&ns(ACTIVATE_NOTIFICATION)),
            None,
            Some(&objc2_foundation::NSOperationQueue::mainQueue()),
            &block,
        )
    };
    // Observer lives for the whole process.
    std::mem::forget(token);
}

// ---------------------------------------------------------------------------
// Main menu (invisible for an agent app, but provides Edit shortcuts)
// ---------------------------------------------------------------------------

fn item(title: &str, action: Option<objc2::runtime::Sel>, key: &str, mods: Option<NSEventModifierFlags>) -> Retained<NSMenuItem> {
    let i = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm()), &ns(title), action, &ns(key)) };
    if let Some(m) = mods {
        i.setKeyEquivalentModifierMask(m);
    }
    i
}

pub fn install_main_menu(m: MainThreadMarker) {
    let app = NSApplication::sharedApplication(m);
    let main = NSMenu::new(m);

    let app_item = NSMenuItem::new(m);
    let app_menu = NSMenu::initWithTitle(NSMenu::alloc(m), &NSString::from_str("Shaft"));
    app_menu.addItem(&item("Quit Shaft", Some(sel!(terminate:)), "q", None));
    app_item.setSubmenu(Some(&app_menu));
    main.addItem(&app_item);

    let edit_item = NSMenuItem::new(m);
    let edit = NSMenu::initWithTitle(NSMenu::alloc(m), &NSString::from_str("Edit"));
    edit.addItem(&item("Undo", Some(sel!(undo:)), "z", None));
    edit.addItem(&item("Redo", Some(sel!(redo:)), "z", Some(NSEventModifierFlags::Command | NSEventModifierFlags::Shift)));
    edit.addItem(&NSMenuItem::separatorItem(m));
    edit.addItem(&item("Cut", Some(sel!(cut:)), "x", None));
    edit.addItem(&item("Copy", Some(sel!(copy:)), "c", None));
    edit.addItem(&item("Paste", Some(sel!(paste:)), "v", None));
    edit.addItem(&item("Select All", Some(sel!(selectAll:)), "a", None));
    edit_item.setSubmenu(Some(&edit));
    main.addItem(&edit_item);

    let window_item = NSMenuItem::new(m);
    let window = NSMenu::initWithTitle(NSMenu::alloc(m), &NSString::from_str("Window"));
    window.addItem(&item("Close", Some(sel!(performClose:)), "w", None));
    window.addItem(&item("Minimize", Some(sel!(performMiniaturize:)), "m", None));
    window_item.setSubmenu(Some(&window));
    main.addItem(&window_item);

    app.setMainMenu(Some(&main));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerators() {
        assert!(parse_accelerator("cmd+shift+n").is_ok());
        assert!(parse_accelerator("cmd+shift+KeyN").is_ok());
        assert!(parse_accelerator("ctrl+option+5").is_ok());
        assert!(parse_accelerator("n").is_err());
        assert!(parse_accelerator("cmd+nonsense").is_err());
    }
}
