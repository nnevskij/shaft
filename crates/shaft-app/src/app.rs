//! Application controller: owns the status item, popover, windows and
//! configuration, and reacts to core events. Lives on the main thread.

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use block2::RcBlock;
use objc2::{rc::Retained, sel};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventMask, NSPopover, NSPopoverBehavior, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
    NSViewController,
};
use objc2_foundation::NSRectEdge;
use shaft_config::{validate_profile, Account, AppConfig, ConfigStore, Protocol, TunnelProfile};
use shaft_core::{CoreError, CoreEvent, CoreHandle, SessionState, TunnelSnapshot, TunnelState};
use uuid::Uuid;

use crate::{
    platform::{on_main, HotkeyAction, Hotkeys, Notifier},
    popover,
    status_icon::{self, IconState},
    ui::{self, mtm, ns, Handlers},
    windows::Windows,
};

thread_local! {
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

/// The running application. Panics if called before [`App::install`].
pub fn app() -> Rc<App> {
    APP.with(|a| a.get().expect("app not initialised").clone())
}

pub fn try_app() -> Option<Rc<App>> {
    APP.with(|a| a.get().cloned())
}

/// State of the quick-tunnel form, kept across popover rebuilds.
#[derive(Debug, Clone)]
pub struct QuickState {
    pub protocol: Protocol,
    pub upstream: String,
}

pub struct App {
    pub core: CoreHandle,
    pub store: ConfigStore,
    pub config: RefCell<AppConfig>,
    status_item: Retained<NSStatusItem>,
    status_handlers: Handlers,
    pub popover: Retained<NSPopover>,
    pub popover_vc: Retained<NSViewController>,
    pub popover_handlers: Handlers,
    pub quick: RefCell<QuickState>,
    pub quick_field: RefCell<Option<Retained<objc2_app_kit::NSTextField>>>,
    pub sessions: RefCell<HashMap<Uuid, SessionState>>,
    /// Last failure per profile, shown in the popover until the next start.
    pub failures: RefCell<HashMap<Uuid, CoreError>>,
    /// Last failed quick tunnel (name, error), shown under the quick form.
    pub quick_error: RefCell<Option<(String, CoreError)>>,
    pub last_url: RefCell<Option<String>>,
    /// Run ids whose URL was already copied/notified.
    announced: RefCell<std::collections::HashSet<Uuid>>,
    render_scheduled: Cell<bool>,
    pub windows: Windows,
    pub notifier: Notifier,
    hotkeys: RefCell<Option<Hotkeys>>,
    popover_monitors: RefCell<Vec<Retained<objc2::runtime::AnyObject>>>,
    pub hotkey_problems: RefCell<Vec<String>>,
    /// A newer release found by the updater, shown in the popover.
    pub update: RefCell<Option<crate::updater::Release>>,
}

impl App {
    pub fn install(core: CoreHandle, store: ConfigStore, config: AppConfig) -> Rc<App> {
        let m = mtm();
        let status_item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let popover = NSPopover::new(m);
        popover.setBehavior(NSPopoverBehavior::ApplicationDefined);
        popover.setAnimates(true);
        let popover_vc = NSViewController::new(m);
        popover.setContentViewController(Some(&popover_vc));

        let app = Rc::new(App {
            core,
            store,
            config: RefCell::new(config),
            status_item,
            status_handlers: Handlers::new(),
            popover,
            popover_vc,
            popover_handlers: Handlers::new(),
            quick: RefCell::new(QuickState { protocol: Protocol::Http, upstream: String::new() }),
            quick_field: RefCell::new(None),
            sessions: RefCell::new(HashMap::new()),
            failures: RefCell::new(HashMap::new()),
            quick_error: RefCell::new(None),
            last_url: RefCell::new(None),
            announced: RefCell::new(Default::default()),
            render_scheduled: Cell::new(false),
            windows: Windows::default(),
            notifier: Notifier::new(),
            hotkeys: RefCell::new(None),
            popover_monitors: RefCell::new(Vec::new()),
            hotkey_problems: RefCell::new(Vec::new()),
            update: RefCell::new(None),
        });
        APP.with(|a| {
            let _ = a.set(app.clone());
        });

        if let Some(button) = app.status_item.button(m) {
            let target = app.status_handlers.add(toggle_popover);
            unsafe {
                button.setTarget(Some(&target));
                button.setAction(Some(sel!(perform:)));
            }
            button.setToolTip(Some(&ns("Shaft")));
        }
        app.update_icon();
        *app.hotkeys.borrow_mut() = Hotkeys::new();
        app.apply_hotkeys();
        app
    }

    // -----------------------------------------------------------------------
    // Config
    // -----------------------------------------------------------------------

    /// Persist the current config; reports failures to the user.
    pub fn save(&self) -> bool {
        let cfg = self.config.borrow().clone();
        match self.store.save(&cfg) {
            Ok(()) => {
                self.core.update_settings(cfg.settings.clone());
                self.apply_hotkeys();
                self.schedule_render();
                true
            }
            Err(e) => {
                tracing::error!(error = %e, "saving config failed");
                ui::error("Could not save settings", &format!("{e:#}"));
                false
            }
        }
    }

    pub fn apply_hotkeys(&self) {
        let cfg = self.config.borrow();
        let hk = &cfg.settings.hotkeys;
        let mut bindings =
            vec![(hk.toggle_popover.clone(), HotkeyAction::TogglePopover), (hk.copy_last_url.clone(), HotkeyAction::CopyLastUrl)];
        for p in &hk.profiles {
            if cfg.profile(p.profile_id).is_some() {
                bindings.push((p.accelerator.clone(), HotkeyAction::ToggleProfile(p.profile_id)));
            }
        }
        drop(cfg);
        let problems = match self.hotkeys.borrow_mut().as_mut() {
            Some(h) => h.set(&bindings),
            None => vec!["global hotkeys are not available on this system".into()],
        };
        for p in &problems {
            tracing::warn!(problem = %p, "hotkey");
        }
        *self.hotkey_problems.borrow_mut() = problems;
    }

    // -----------------------------------------------------------------------
    // Tunnels
    // -----------------------------------------------------------------------

    pub fn start_profile(&self, profile_id: Uuid) {
        let (profile, account) = {
            let cfg = self.config.borrow();
            let Some(p) = cfg.profile(profile_id).cloned() else { return };
            let Some(a) = cfg.account(p.account_id).cloned() else {
                drop(cfg);
                ui::error("Missing account", "This profile's account no longer exists. Edit the profile and pick an account.");
                return;
            };
            (p, a)
        };
        let errors = validate_profile(&profile);
        if !errors.is_empty() {
            ui::error(&format!("Cannot start “{}”", profile.name), &errors.join("\n"));
            return;
        }
        self.failures.borrow_mut().remove(&profile_id);
        self.core.start_profile(profile, account);
    }

    pub fn toggle_profile(&self, profile_id: Uuid) {
        if self.core.running_profile(profile_id).is_some() {
            self.core.stop_profile(profile_id);
        } else {
            self.start_profile(profile_id);
        }
    }

    pub fn start_quick(&self) {
        if let Some(f) = self.quick_field.borrow().as_ref() {
            self.quick.borrow_mut().upstream = ui::value(f);
        }
        let q = self.quick.borrow().clone();
        if q.upstream.is_empty() {
            ui::error("Nothing to expose", "Enter a port (e.g. 3000), host:port or URL of the local service.");
            return;
        }
        let Some(account) = self.config.borrow().active_account().cloned() else {
            ui::error("No account", "Add an ngrok account first.");
            return;
        };
        let mut p = TunnelProfile::new(format!("{} {}", q.protocol.label(), q.upstream), account.id, q.protocol, q.upstream.clone());
        p.inspect = true;
        let errors = validate_profile(&p);
        if !errors.is_empty() {
            ui::error("Cannot start tunnel", &errors.join("\n"));
            return;
        }
        *self.quick_error.borrow_mut() = None;
        self.core.start_profile(p, account);
        self.schedule_render();
    }

    pub fn copy_url(&self, url: &str) {
        ui::copy_to_clipboard(url);
        *self.last_url.borrow_mut() = Some(url.to_string());
        if self.config.borrow().settings.notifications.url_copied {
            self.notifier.notify("URL copied", url);
        }
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    pub fn handle_event(&self, ev: CoreEvent) {
        let notif = self.config.borrow().settings.notifications.clone();
        match &ev {
            CoreEvent::TunnelChanged(s) => {
                if s.state.is_online() {
                    if let Some(url) = &s.public_url {
                        let first = self.announced.borrow_mut().insert(s.run_id);
                        *self.last_url.borrow_mut() = Some(url.clone());
                        if first {
                            if self.config.borrow().settings.copy_url_on_start {
                                ui::copy_to_clipboard(url);
                            }
                            if notif.tunnel_online {
                                self.notifier.notify(&format!("{} is online", s.name), url);
                            }
                        }
                    }
                }
                if let TunnelState::Failed(e) = &s.state {
                    self.failures.borrow_mut().insert(s.profile_id, e.clone());
                }
            }
            CoreEvent::TunnelRemoved { run_id, profile_id, name, failed } => {
                let was_online = self.announced.borrow_mut().remove(run_id);
                if let Some(e) = failed {
                    self.failures.borrow_mut().insert(*profile_id, e.clone());
                    // Quick tunnels have no saved profile row to show the error under.
                    if self.config.borrow().profile(*profile_id).is_none() {
                        *self.quick_error.borrow_mut() = Some((name.clone(), e.clone()));
                    }
                    if notif.errors || (was_online && notif.tunnel_down) {
                        self.notifier.notify(&format!("{name} stopped"), &e.to_string());
                    }
                }
                self.windows.inspector_runs_changed();
            }
            CoreEvent::SessionChanged { account_id, state } => {
                let prev = self.sessions.borrow_mut().insert(*account_id, state.clone());
                let name = self.config.borrow().account(*account_id).map(|a| a.name.clone()).unwrap_or_default();
                match state {
                    SessionState::Reconnecting { attempt: 1, error } if notif.tunnel_down => {
                        self.notifier.notify(&format!("Connection lost ({name})"), &format!("Reconnecting… {error}"));
                    }
                    SessionState::Connected if matches!(prev, Some(SessionState::Reconnecting { .. })) && notif.tunnel_reconnected => {
                        self.notifier.notify(&format!("Reconnected ({name})"), "Tunnels are back online.");
                    }
                    SessionState::Failed(e) if notif.errors => {
                        self.notifier.notify(&format!("Connection failed ({name})"), &e.to_string());
                    }
                    _ => {}
                }
            }
            CoreEvent::InspectorChanged { run_id } => {
                self.windows.inspector_changed(*run_id);
            }
            CoreEvent::RemoteStop { account_id } => {
                let name = self.config.borrow().account(*account_id).map(|a| a.name.clone()).unwrap_or_default();
                self.notifier.notify("Tunnels stopped", &format!("Account “{name}”: all tunnels were stopped."));
            }
        }
        if matches!(ev, CoreEvent::TunnelChanged(_)) && self.windows.inspector_open() {
            self.windows.inspector_runs_changed();
        }
        self.update_icon();
        if !matches!(ev, CoreEvent::InspectorChanged { .. }) || self.popover.isShown() {
            self.schedule_render();
        }
    }

    pub fn update_icon(&self) {
        let snaps = self.core.snapshots();
        let online = snaps.iter().filter(|s| s.state.is_online()).count();
        let failed = snaps.iter().any(|s| matches!(s.state, TunnelState::Failed(_)));
        let busy = snaps.iter().any(|s| matches!(s.state, TunnelState::Starting | TunnelState::Reconnecting { .. }));
        let state = if failed {
            IconState::Error
        } else if busy {
            IconState::Connecting
        } else if online > 0 {
            IconState::Online
        } else {
            IconState::Idle
        };
        let Some(button) = self.status_item.button(mtm()) else { return };
        button.setImage(Some(&status_icon::image(state)));
        let title = if online > 0 { format!(" {online}") } else { String::new() };
        if button.title().to_string() != title {
            button.setTitle(&ns(&title));
            // The new title changes the item's width; once the status bar has
            // laid it out again, re-anchor an open popover to the button.
            if self.popover.isShown() {
                on_main(|| {
                    let Some(app) = try_app() else { return };
                    if let (true, Some(button)) = (app.popover.isShown(), app.status_item.button(mtm())) {
                        app.popover.setPositioningRect(button.bounds());
                    }
                });
            }
        }
        button.setToolTip(Some(&ns(&tooltip(&snaps))));
    }

    /// Coalesce re-renders to once per run-loop turn.
    pub fn schedule_render(&self) {
        if self.render_scheduled.replace(true) {
            return;
        }
        on_main(|| {
            let Some(app) = try_app() else { return };
            app.render_scheduled.set(false);
            if app.popover.isShown() {
                popover::render(&app);
            }
            app.windows.refresh_profiles_dependent();
        });
    }

    pub fn close_popover(&self) {
        self.remove_popover_monitors();
        if self.popover.isShown() {
            unsafe { self.popover.performClose(None) };
        }
    }

    pub fn quit(&self) {
        self.close_popover();
        self.core.shutdown_blocking(Duration::from_secs(3));
        NSApplication::sharedApplication(mtm()).terminate(None);
    }
}

fn tooltip(snaps: &[TunnelSnapshot]) -> String {
    if snaps.is_empty() {
        return "Shaft — no active tunnels".into();
    }
    let mut s = String::from("Shaft");
    for t in snaps {
        s.push_str(&format!("\n{} — {}", t.name, t.public_url.as_deref().unwrap_or(t.state.label())));
    }
    s
}

pub fn toggle_popover() {
    let app = app();
    if app.popover.isShown() {
        app.close_popover();
    } else {
        show_popover();
    }
}

pub fn show_popover() {
    show_popover_attempt(0);
}

fn show_popover_attempt(attempt: u32) {
    let Some(app) = try_app() else { return };
    let m = mtm();
    popover::render(&app);
    if let Some(button) = app.status_item.button(m) {
        #[allow(deprecated)]
        NSApplication::sharedApplication(m).activateIgnoringOtherApps(true);
        app.popover.showRelativeToRect_ofView_preferredEdge(button.bounds(), &button, NSRectEdge::MinY);
        if let Some(w) = app.popover.contentViewController().and_then(|vc| vc.view().window()) {
            w.makeKeyWindow();
        }
        app.install_popover_monitors();
        // Right after launch the status item may not be placed yet.
        if !app.popover.isShown() && attempt < 5 {
            tracing::debug!(attempt, "popover not shown yet, retrying");
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                on_main(move || show_popover_attempt(attempt + 1));
            });
        }
    }
}

impl App {
    /// The popover is application-defined (a transient popover closes
    /// immediately when macOS refuses to activate the app, e.g. from a global
    /// shortcut), so close it ourselves on outside clicks and Escape.
    fn install_popover_monitors(&self) {
        if !self.popover_monitors.borrow().is_empty() {
            return;
        }
        let mut monitors = Vec::new();
        let outside = RcBlock::new(|_e: std::ptr::NonNull<NSEvent>| {
            if let Some(app) = try_app() {
                app.close_popover();
            }
        });
        if let Some(m) =
            NSEvent::addGlobalMonitorForEventsMatchingMask_handler(NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown, &outside)
        {
            monitors.push(m);
        }
        let escape = RcBlock::new(|e: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { e.as_ref() };
            if event.keyCode() == 53 {
                if let Some(app) = try_app() {
                    if app.popover.isShown() {
                        app.close_popover();
                        return std::ptr::null_mut();
                    }
                }
            }
            e.as_ptr()
        });
        if let Some(m) = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &escape) } {
            monitors.push(m);
        }
        *self.popover_monitors.borrow_mut() = monitors;
    }

    fn remove_popover_monitors(&self) {
        for m in self.popover_monitors.borrow_mut().drain(..) {
            unsafe { NSEvent::removeMonitor(&m) };
        }
    }
}

pub fn hotkey_pressed(id: u32) {
    let Some(app) = try_app() else { return };
    let action = app.hotkeys.borrow().as_ref().and_then(|h| h.action(id));
    match action {
        Some(HotkeyAction::TogglePopover) => toggle_popover(),
        Some(HotkeyAction::CopyLastUrl) => {
            let url = app.last_url.borrow().clone();
            if let Some(url) = url {
                app.copy_url(&url);
            }
        }
        Some(HotkeyAction::ToggleProfile(pid)) => app.toggle_profile(pid),
        None => {}
    }
}

/// Event sink handed to the core: hops to the main thread.
pub fn event_sink() -> shaft_core::EventSink {
    Arc::new(|ev: CoreEvent| {
        on_main(move || {
            if let Some(app) = try_app() {
                app.handle_event(ev);
            }
        })
    })
}

/// A new account built from onboarding input.
pub fn new_account(name: &str, authtoken: &str) -> Account {
    let mut a = Account::new(if name.trim().is_empty() { "Personal" } else { name.trim() });
    a.authtoken = authtoken.trim().into();
    a
}
