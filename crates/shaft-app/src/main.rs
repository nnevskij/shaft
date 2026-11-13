//! Shaft — a menu bar client for ngrok.

mod app;
mod kit;
mod logging;
mod platform;
mod popover;
mod ui;
mod updater;
mod windows;

use std::sync::Arc;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use shaft_config::{ConfigStore, SecretStore};

fn main() {
    let mtm = MainThreadMarker::new().expect("must start on the main thread");

    let secrets: Arc<dyn SecretStore> = if std::env::var_os("SHAFT_NO_KEYCHAIN").is_some() {
        Arc::new(shaft_config::MemorySecretStore::new())
    } else {
        Arc::new(shaft_config::KeychainSecretStore::new())
    };
    let store = match std::env::var_os("SHAFT_CONFIG") {
        Some(p) => ConfigStore::new(p, secrets),
        None => ConfigStore::default_location(secrets),
    };
    let (config, load_error) = match store.load() {
        Ok(c) => (c, None),
        Err(e) => (shaft_config::AppConfig::default(), Some(format!("{e:#}"))),
    };
    let _log_guard = logging::init(&config.settings.log_level);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), config = %store.path().display(), "starting Shaft");

    let Some(lock) = platform::acquire_instance_lock(store.path()) else {
        tracing::info!("another instance is running; asked it to show itself");
        return;
    };

    let runtime =
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("shaft-rt").build().expect("tokio runtime");

    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    platform::install_main_menu(mtm);

    let core = shaft_core::CoreHandle::start(runtime.handle().clone(), config.settings.clone(), app::event_sink());
    let first_run = config.accounts.is_empty();
    let app = app::App::install(core, store, config);
    platform::observe_activation_requests(&lock);
    updater::start();

    if let Some(err) = load_error {
        // Do not overwrite a config we could not read.
        tracing::error!(error = %err, "config could not be loaded");
        ui::error(
            "Shaft could not read its settings",
            &format!("{err}\n\nShaft started with empty settings; your file was left untouched. Fix or remove it and restart."),
        );
    }
    if first_run {
        platform::on_main(app::show_popover);
    }
    // Development aid: open windows at launch, e.g. SHAFT_DEBUG_OPEN=popover,inspector
    if let Ok(list) = std::env::var("SHAFT_DEBUG_OPEN") {
        platform::on_main(move || {
            for name in list.split(',').map(str::trim) {
                match name {
                    "popover" => app::show_popover(),
                    "profile" => windows::profile::open(None),
                    "edit-profile" => {
                        let id = app::app().config.borrow().profiles.first().map(|p| p.id);
                        windows::profile::open(id)
                    }
                    "accounts" => windows::accounts::open(),
                    "inspector" => windows::inspector::open(None),
                    "resources" => windows::resources::open(),
                    "settings" => windows::settings::open(),
                    "diagnostics" => windows::diagnostics::open(),
                    "check-updates" => updater::check(updater::Trigger::Manual),
                    "start-all" => {
                        let ids: Vec<_> = app::app().config.borrow().profiles.iter().map(|p| p.id).collect();
                        for id in ids {
                            app::app().start_profile(id);
                        }
                    }
                    q if q.starts_with("quick:") => {
                        let a = app::app();
                        let upstream = q.trim_start_matches("quick:");
                        a.quick.borrow_mut().upstream = upstream.to_string();
                        if let Some(f) = a.quick_field.borrow().as_ref() {
                            f.setStringValue(&objc2_foundation::NSString::from_str(upstream));
                        }
                        a.start_quick();
                    }
                    other => tracing::warn!(other, "unknown SHAFT_DEBUG_OPEN entry"),
                }
            }
        });
    }
    drop(app);

    ns_app.run();
    drop(runtime);
}
