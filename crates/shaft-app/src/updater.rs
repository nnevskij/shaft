//! Update detection. Shaft asks GitHub for the latest published release at
//! launch and then once a day, and offers it when it is newer than the running
//! build: a notification, a banner in the popover and an alert on manual
//! checks. "Install and Relaunch" downloads and verifies the DMG, replaces the
//! bundle and restarts Shaft (see [`crate::installer`]); when Shaft cannot
//! replace itself (no write access, unsigned build…) it opens the download.

use std::{
    cell::{Cell, OnceCell, RefCell},
    cmp::Ordering,
    fmt,
    sync::{
        atomic::{AtomicU8, Ordering as AtomicOrdering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use serde::Deserialize;

use crate::{
    app::{app, try_app},
    installer,
    platform::on_main,
    ui,
};

/// GitHub "latest release" endpoint (drafts and pre-releases are excluded).
const FEED: &str = "https://api.github.com/repos/nnevskij/shaft/releases/latest";
/// Delay before the first automatic check, so it does not compete with launch.
const FIRST_DELAY: Duration = Duration::from_secs(15);
/// How often the scheduler wakes up to see whether a check is due.
const TICK: Duration = Duration::from_secs(60 * 60);
/// Minimum time between automatic checks.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// Versions and releases
// ---------------------------------------------------------------------------

/// A `major.minor.patch[-pre]` version, ordered by SemVer precedence
/// (a pre-release sorts before the release it precedes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next()?; // build metadata does not affect precedence
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut parts = core.split('.');
        let mut num = || parts.next().map(|p| p.parse::<u64>().ok()).unwrap_or(Some(0));
        let (major, minor, patch) = (num()?, num()?, num()?);
        if parts.next().is_some() || core.is_empty() {
            return None;
        }
        Some(Version { major, minor, patch, pre })
    }

    /// The version of this build.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is valid semver")
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => cmp_pre(a, b),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// SemVer pre-release precedence: dot-separated identifiers, numeric ones
/// compared numerically and lower than alphanumeric ones.
fn cmp_pre(a: &str, b: &str) -> Ordering {
    let mut ia = a.split('.');
    let mut ib = b.split('.');
    loop {
        match (ia.next(), ib.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Release page (notes, all assets).
    pub page_url: String,
    /// Direct link to the DMG, when the release has one.
    pub download_url: Option<String>,
    /// The DMG's `.sha256` file, when published.
    pub checksum_url: Option<String>,
    pub notes: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Parse a GitHub release object.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let r: GhRelease = serde_json::from_str(json).map_err(|e| format!("unexpected response from GitHub: {e}"))?;
    if r.draft || r.prerelease {
        return Err("the latest release is not a final release".into());
    }
    let version = Version::parse(&r.tag_name).ok_or_else(|| format!("release tag `{}` is not a version", r.tag_name))?;
    let asset =
        |suffix: &str| r.assets.iter().find(|a| a.name.to_ascii_lowercase().ends_with(suffix)).map(|a| a.browser_download_url.clone());
    let (download_url, checksum_url) = (asset(".dmg"), asset(".dmg.sha256"));
    Ok(Release { version, page_url: r.html_url, download_url, checksum_url, notes: r.body.unwrap_or_default().trim().to_string() })
}

fn feed_url() -> String {
    std::env::var("SHAFT_UPDATE_URL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| FEED.to_string())
}

async fn fetch(url: &str) -> Result<Release, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("Shaft/", env!("CARGO_PKG_VERSION"), " (macOS; update check)"))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub: {e}"))?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Err("no release has been published yet".into()),
        403 | 429 => return Err("GitHub rate limit reached; try again later".into()),
        s => return Err(format!("GitHub answered HTTP {s}")),
    }
    let body = resp.text().await.map_err(|e| format!("could not read GitHub's answer: {e}"))?;
    parse_release(&body)
}

// ---------------------------------------------------------------------------
// Checking (main thread)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Scheduled: silent unless something new was found.
    Automatic,
    /// From the UI: always reports the outcome.
    Manual,
    /// Development aid (`SHAFT_DEBUG_OPEN=update-now`): install whatever is
    /// newer without asking.
    InstallNow,
}

thread_local! {
    static CHECKING: Cell<bool> = const { Cell::new(false) };
    static LAST_CHECK: Cell<Option<SystemTime>> = const { Cell::new(None) };
    static LAST_OUTCOME: RefCell<String> = const { RefCell::new(String::new()) };
    /// Version already announced with a notification in this run.
    static NOTIFIED: RefCell<Option<Version>> = const { RefCell::new(None) };
}

/// Start the background schedule. Automatic checks only run from the app
/// bundle (not `cargo run`), unless `SHAFT_UPDATE_URL` points somewhere.
pub fn start() {
    let bundled = objc2_foundation::NSBundle::mainBundle().bundleIdentifier().is_some();
    if !bundled && std::env::var_os("SHAFT_UPDATE_URL").is_none() {
        tracing::debug!("not running from an app bundle: automatic update checks disabled");
        return;
    }
    let rt = app().core.runtime().clone();
    rt.spawn(async {
        tokio::time::sleep(FIRST_DELAY).await;
        loop {
            on_main(check_if_due);
            tokio::time::sleep(TICK).await;
        }
    });
}

fn check_if_due() {
    let Some(app) = try_app() else { return };
    if !app.config.borrow().settings.updates.check_automatically {
        return;
    }
    // Wall-clock time, so a Mac that slept for days checks on wake.
    let due = LAST_CHECK.with(|l| l.get()).and_then(|t| t.elapsed().ok()).is_none_or(|e| e >= INTERVAL);
    if due {
        check(Trigger::Automatic);
    }
}

pub fn is_checking() -> bool {
    CHECKING.with(|c| c.get())
}

/// One-line description of the last check, for the Settings pane.
pub fn status() -> String {
    if is_checking() {
        return "Checking…".into();
    }
    LAST_OUTCOME.with(|o| o.borrow().clone())
}

pub fn check(trigger: Trigger) {
    let Some(app) = try_app() else { return };
    if CHECKING.with(|c| c.replace(true)) {
        return;
    }
    LAST_CHECK.with(|l| l.set(Some(SystemTime::now())));
    crate::windows::settings::refresh_updates();
    let url = feed_url();
    tracing::info!(?trigger, url, "checking for updates");
    app.core.runtime().spawn(async move {
        let result = fetch(&url).await;
        on_main(move || finish(trigger, result));
    });
}

fn finish(trigger: Trigger, result: Result<Release, String>) {
    CHECKING.with(|c| c.set(false));
    let Some(app) = try_app() else { return };
    let current = Version::current();
    let outcome = match result {
        Ok(rel) if rel.version > current => {
            tracing::info!(latest = %rel.version, %current, "update available");
            let skipped = app.config.borrow().settings.updates.skipped_version.as_deref() == Some(&rel.version.to_string());
            let outcome = format!("Shaft {} is available.", rel.version);
            if skipped && trigger == Trigger::Automatic {
                tracing::info!(version = %rel.version, "update skipped by the user");
                set_outcome(format!("{outcome} (skipped)"));
                return;
            }
            *app.update.borrow_mut() = Some(rel.clone());
            app.schedule_render();
            set_outcome(outcome);
            match trigger {
                Trigger::Manual => prompt(&rel),
                Trigger::InstallNow => install(&rel),
                Trigger::Automatic => {
                    let first = NOTIFIED.with(|n| n.replace(Some(rel.version.clone())).as_ref() != Some(&rel.version));
                    if first {
                        app.notifier.notify(&format!("Shaft {} is available", rel.version), "Open Shaft from the menu bar to update.");
                    }
                }
            }
            return;
        }
        Ok(rel) => {
            tracing::info!(latest = %rel.version, %current, "up to date");
            if app.update.borrow_mut().take().is_some() {
                app.schedule_render();
            }
            if trigger == Trigger::Manual {
                ui::info("Shaft is up to date", &format!("Version {current} is the latest release."));
            }
            format!("Up to date (checked {}).", clock())
        }
        Err(e) => {
            tracing::warn!(error = %e, "update check failed");
            if trigger == Trigger::Manual {
                ui::error("Could not check for updates", &e);
            }
            format!("Last check failed: {e}")
        }
    };
    set_outcome(outcome);
}

fn set_outcome(s: String) {
    LAST_OUTCOME.with(|o| *o.borrow_mut() = s);
    crate::windows::settings::refresh_updates();
}

/// Local time of day as `HH:MM`.
fn clock() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// Ask what to do with `rel`: install it, skip this version, or decide later.
pub fn prompt(rel: &Release) {
    let mut info = format!("You have version {}.", Version::current());
    if !rel.notes.is_empty() {
        let mut notes: String = rel.notes.chars().take(700).collect();
        if notes.len() < rel.notes.len() {
            notes.push('…');
        }
        info.push_str(&format!("\n\n{notes}"));
    }
    let primary = if self_update_blocker().is_none() { "Install and Relaunch" } else { "Download" };
    match ui::choose(&format!("Shaft {} is available", rel.version), &info, &[primary, "Skip This Version", "Later"]) {
        0 => install(rel),
        1 => skip(rel),
        _ => {}
    }
}

/// Open the DMG download (or the release page) in the browser.
pub fn download(rel: &Release) {
    ui::open_url(rel.download_url.as_deref().unwrap_or(&rel.page_url));
}

// ---------------------------------------------------------------------------
// Installing (main thread)
// ---------------------------------------------------------------------------

/// Progress of an in-app update, shown in the popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    Idle,
    /// Percent downloaded, when the size is known.
    Downloading(Option<u8>),
    /// Verifying and staging the new bundle.
    Installing,
}

thread_local! {
    static INSTALL: Cell<Install> = const { Cell::new(Install::Idle) };
    /// Why this copy cannot update itself (`None` = it can), computed once.
    static BLOCKER: OnceCell<Option<String>> = const { OnceCell::new() };
}

pub fn install_state() -> Install {
    INSTALL.with(|s| s.get())
}

fn set_install_state(state: Install) {
    if INSTALL.with(|s| s.replace(state)) != state {
        if let Some(app) = try_app() {
            app.schedule_render();
        }
    }
}

/// Why Shaft cannot replace itself here, if it cannot.
pub fn self_update_blocker() -> Option<String> {
    BLOCKER.with(|b| b.get_or_init(|| installer::target().err()).clone())
}

/// Download, verify and install `rel`, then quit; the helper relaunches Shaft.
pub fn install(rel: &Release) {
    if install_state() != Install::Idle {
        return;
    }
    let target = match installer::target() {
        Ok(t) => t,
        Err(why) => {
            tracing::info!(reason = %why, "self-update not possible, opening the download");
            if ui::choose("Shaft can't update itself", &format!("{why}\n\nDownload the new version instead?"), &["Download", "Cancel"]) == 0
            {
                download(rel);
            }
            return;
        }
    };
    let Some(url) = rel.download_url.clone() else {
        download(rel);
        return;
    };
    let app = app();
    let active = app.core.snapshots().len();
    if active > 0 {
        let tunnels = if active == 1 { "1 active tunnel".to_string() } else { format!("{active} active tunnels") };
        let ok = ui::confirm(
            &format!("Install Shaft {} and relaunch?", rel.version),
            &format!("Shaft will quit while it updates: {tunnels} will stop and won't restart on their own."),
            "Install and Relaunch",
            false,
        );
        if !ok {
            return;
        }
    }

    tracing::info!(version = %rel.version, url, "installing update");
    set_install_state(Install::Downloading(None));
    let version = rel.version.to_string();
    let checksum_url = rel.checksum_url.clone();
    let rel_c = rel.clone();
    app.core.runtime().spawn(async move {
        let result = async {
            let dir = installer::work_dir()?;
            let dmg = dir.join(format!("Shaft-{version}.dmg"));
            let last = Arc::new(AtomicU8::new(u8::MAX));
            installer::download(&url, &dmg, |received, total| {
                let pct = total.filter(|t| *t > 0).map(|t| (received * 100 / t).min(100) as u8);
                // Hop to the main thread only when the shown value changes.
                if last.swap(pct.unwrap_or(u8::MAX - 1), AtomicOrdering::Relaxed) != pct.unwrap_or(u8::MAX - 1) {
                    on_main(move || set_install_state(Install::Downloading(pct)));
                }
            })
            .await?;
            let sha = match &checksum_url {
                Some(u) => Some(installer::fetch_checksum(u).await?),
                None => None,
            };
            on_main(|| set_install_state(Install::Installing));
            let staged =
                tokio::task::spawn_blocking(move || installer::stage(&target, &dmg, &version, sha.as_deref()).map(|s| (target, s)))
                    .await
                    .map_err(|e| e.to_string())??;
            Ok::<_, String>(staged)
        }
        .await;
        on_main(move || finish_install(&rel_c, result));
    });
}

fn finish_install(rel: &Release, result: Result<(installer::Target, std::path::PathBuf), String>) {
    let spawned = result.and_then(|(target, staged)| {
        let log = shaft_config::log_dir().join("update.log");
        installer::spawn_swap(&target, &staged, &log).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&staged);
        })
    });
    match spawned {
        Ok(()) => {
            tracing::info!(version = %rel.version, "update staged; quitting to relaunch");
            app().quit();
        }
        Err(e) => {
            tracing::error!(error = %e, "update failed");
            set_install_state(Install::Idle);
            if ui::choose(
                "Shaft could not be updated",
                &format!("{e}\n\nYou can download and install it manually."),
                &["Download", "Cancel"],
            ) == 0
            {
                download(rel);
            }
        }
    }
}

pub fn skip(rel: &Release) {
    let app = app();
    app.config.borrow_mut().settings.updates.skipped_version = Some(rel.version.to_string());
    app.update.borrow_mut().take();
    app.save();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s} should parse"))
    }

    #[test]
    fn parses_versions() {
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert_eq!(v("1.2").to_string(), "1.2.0");
        assert_eq!(v("2").to_string(), "2.0.0");
        assert_eq!(v("1.0.0-beta.2+build.5").to_string(), "1.0.0-beta.2");
        for bad in ["", "v", "1.x", "1.2.3.4", "1.2.3-", "latest"] {
            assert!(Version::parse(bad).is_none(), "{bad} should not parse");
        }
    }

    #[test]
    fn orders_versions() {
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.2"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha.9"));
        assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
        assert_eq!(v("v1.2.0").cmp(&v("1.2")), Ordering::Equal);
    }

    #[test]
    fn current_version_parses() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn parses_github_release() {
        let json = r#"{
            "tag_name": "v0.3.1",
            "html_url": "https://github.com/nnevskij/shaft/releases/tag/v0.3.1",
            "body": "  Fixes.  ",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": "Shaft-0.3.1.dmg.sha256", "browser_download_url": "https://x/sha"},
                {"name": "Shaft-0.3.1.dmg", "browser_download_url": "https://x/dmg"}
            ]
        }"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, v("0.3.1"));
        assert_eq!(r.download_url.as_deref(), Some("https://x/dmg"));
        assert_eq!(r.checksum_url.as_deref(), Some("https://x/sha"));
        assert_eq!(r.notes, "Fixes.");

        let no_assets = r#"{"tag_name": "v1.0.0", "html_url": "https://x/page"}"#;
        assert_eq!(parse_release(no_assets).unwrap().download_url, None);
        assert!(parse_release(r#"{"tag_name": "nightly", "html_url": "x"}"#).is_err());
        assert!(parse_release(r#"{"tag_name": "v2.0.0", "html_url": "x", "prerelease": true}"#).is_err());
        assert!(parse_release("not json").is_err());
    }
}
