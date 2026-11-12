//! File logging with rotation and a runtime-adjustable level.

use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, reload, util::SubscriberInitExt, EnvFilter, Registry};

static RELOAD: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

fn filter(level: &str) -> EnvFilter {
    // Keep noisy dependencies quieter than our own crates.
    EnvFilter::try_new(format!(
        "warn,shaft={level},shaft_core={level},shaft_inspector={level},shaft_api={level},shaft_config={level},ngrok=warn"
    ))
    .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Log to `~/Library/Logs/dev.shaft.app/shaft.log.YYYY-MM-DD` (7 files kept)
/// and to stderr. `SHAFT_LOG` overrides the level.
pub fn init(level: &str) -> Option<WorkerGuard> {
    let level = std::env::var("SHAFT_LOG").unwrap_or_else(|_| level.to_string());
    let (filter_layer, handle) = reload::Layer::new(filter(&level));
    let _ = RELOAD.set(handle);
    let dir = shaft_config::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("shaft.log")
        .max_log_files(7)
        .build(&dir)
        .ok();
    let registry = tracing_subscriber::registry().with(filter_layer).with(fmt::layer().with_writer(std::io::stderr).with_target(false));
    match file {
        Some(appender) => {
            let (nb, guard) = tracing_appender::non_blocking(appender);
            registry.with(fmt::layer().with_writer(nb).with_ansi(false)).init();
            Some(guard)
        }
        None => {
            registry.init();
            tracing::warn!(dir = %dir.display(), "file logging unavailable");
            None
        }
    }
}

pub fn set_level(level: &str) {
    if let Some(h) = RELOAD.get() {
        let _ = h.modify(|f| *f = filter(level));
    }
}
