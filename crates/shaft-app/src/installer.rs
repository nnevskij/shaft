//! In-place self-update: download a release DMG, verify it, stage the new
//! bundle next to the running one, and swap the two from a small helper
//! script once Shaft has quit, then relaunch.
//!
//! The new bundle is only accepted when it passes `codesign --verify`, is
//! signed by the same Team ID as the running app, has the same bundle
//! identifier and the expected version, and is accepted by Gatekeeper
//! (notarized). The DMG's SHA-256 is checked against the release's
//! `.sha256` asset when there is one.

use std::{
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Where the running bundle lives and who signed it.
#[derive(Debug, Clone)]
pub struct Target {
    pub bundle: PathBuf,
    pub team_id: String,
    pub bundle_id: String,
}

/// Can this copy of Shaft replace itself? Returns why not otherwise.
pub fn target() -> Result<Target, String> {
    let path = objc2_foundation::NSBundle::mainBundle().bundlePath().to_string();
    let bundle = PathBuf::from(&path);
    if bundle.extension().and_then(|e| e.to_str()) != Some("app") {
        return Err("Shaft is not running from an app bundle.".into());
    }
    if path.contains("/AppTranslocation/") || path.starts_with("/Volumes/") {
        return Err("Shaft is running from the disk image or a quarantined location. Move it to the Applications folder first.".into());
    }
    let parent = bundle.parent().ok_or("Shaft's folder could not be determined.")?;
    if !writable(parent) || !writable(&bundle) {
        return Err(format!("You don't have permission to replace Shaft in {}.", parent.display()));
    }
    let team_id = team_id(&bundle)?.ok_or("This copy of Shaft is not signed with a Developer ID, so updates can't be verified.")?;
    let bundle_id = plist_value(&bundle, "CFBundleIdentifier")?;
    Ok(Target { bundle, team_id, bundle_id })
}

fn writable(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Run a tool and return stdout, or an error with its stderr.
fn run(cmd: &str, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new(cmd).args(args).stdin(Stdio::null()).output().map_err(|e| format!("could not run {cmd}: {e}"))
}

fn run_ok(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = run(cmd, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let name = Path::new(cmd).file_name().and_then(|n| n.to_str()).unwrap_or(cmd);
        Err(format!("{name} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The signing Team ID of a bundle (`None` when ad-hoc signed or unsigned).
fn team_id(bundle: &Path) -> Result<Option<String>, String> {
    let out = run("/usr/bin/codesign", &["-dv", "--verbose=2", &bundle.to_string_lossy()])?;
    // codesign prints signature details on stderr.
    let text = String::from_utf8_lossy(&out.stderr);
    Ok(parse_team_id(&text))
}

pub fn parse_team_id(codesign_output: &str) -> Option<String> {
    codesign_output
        .lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "not set")
        .map(str::to_string)
}

fn plist_value(bundle: &Path, key: &str) -> Result<String, String> {
    let plist = bundle.join("Contents/Info.plist");
    run_ok("/usr/bin/plutil", &["-extract", key, "raw", "-o", "-", &plist.to_string_lossy()]).map(|s| s.trim().to_string())
}

/// `~/Library/Caches/dev.shaft.app/updates`, emptied before each download.
pub fn work_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let dir = PathBuf::from(home).join("Library/Caches").join(shaft_config::BUNDLE_ID).join("updates");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("Shaft/", env!("CARGO_PKG_VERSION"), " (macOS; updater)"))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

/// Download `url` to `dest`, reporting (received, total) as it goes.
pub async fn download(url: &str, dest: &Path, progress: impl Fn(u64, Option<u64>)) -> Result<(), String> {
    let mut resp = client()?.get(url).send().await.map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status().as_u16()));
    }
    let total = resp.content_length();
    let mut file = std::fs::File::create(dest).map_err(|e| format!("could not write {}: {e}", dest.display()))?;
    let mut received = 0u64;
    progress(0, total);
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("download interrupted: {e}"))? {
        file.write_all(&chunk).map_err(|e| format!("could not write the update: {e}"))?;
        received += chunk.len() as u64;
        progress(received, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    if total.is_some_and(|t| t != received) {
        return Err("the download is incomplete".into());
    }
    Ok(())
}

/// The hex SHA-256 published next to the DMG (`<hash>  <file>` format).
pub async fn fetch_checksum(url: &str) -> Result<String, String> {
    let resp = client()?.get(url).send().await.map_err(|e| format!("could not fetch the checksum: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("could not fetch the checksum: HTTP {}", resp.status().as_u16()));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    parse_checksum(&text).ok_or_else(|| "the published checksum is malformed".into())
}

pub fn parse_checksum(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?.to_ascii_lowercase();
    (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then_some(hash)
}

/// Verify the DMG, copy the app out of it next to the running bundle and
/// check the copy. Returns the staged bundle. Blocking.
pub fn stage(target: &Target, dmg: &Path, version: &str, sha256: Option<&str>) -> Result<PathBuf, String> {
    if let Some(expected) = sha256 {
        let out = run_ok("/usr/bin/shasum", &["-a", "256", &dmg.to_string_lossy()])?;
        let actual = parse_checksum(&out).ok_or("could not hash the download")?;
        if actual != expected {
            return Err("the download does not match the published checksum".into());
        }
    }

    let mount = dmg.with_extension("mnt");
    std::fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
    let m = mount.to_string_lossy().into_owned();
    run_ok("/usr/bin/hdiutil", &["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint", &m, &dmg.to_string_lossy()])?;
    let staged = target.bundle.with_file_name(".Shaft-update.app");
    let copied = (|| {
        let app = std::fs::read_dir(&mount)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
            .ok_or("the disk image does not contain an app")?;
        let _ = std::fs::remove_dir_all(&staged);
        run_ok("/usr/bin/ditto", &[&app.to_string_lossy(), &staged.to_string_lossy()]).map(|_| ())
    })();
    let _ = run("/usr/bin/hdiutil", &["detach", "-force", &m]);
    let _ = std::fs::remove_dir(&mount);
    copied?;

    if let Err(e) = verify(target, &staged, version) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e);
    }
    Ok(staged)
}

fn verify(target: &Target, staged: &Path, version: &str) -> Result<(), String> {
    let s = staged.to_string_lossy();
    run_ok("/usr/bin/codesign", &["--verify", "--deep", "--strict", &s])
        .map_err(|e| format!("the new version's signature is invalid ({e})"))?;
    match team_id(staged)? {
        Some(t) if t == target.team_id => {}
        other => return Err(format!("the new version is signed by another developer ({})", other.as_deref().unwrap_or("unsigned"))),
    }
    let id = plist_value(staged, "CFBundleIdentifier")?;
    if id != target.bundle_id {
        return Err(format!("the disk image contains a different app ({id})"));
    }
    let v = plist_value(staged, "CFBundleShortVersionString")?;
    if v != version {
        return Err(format!("the disk image contains version {v}, expected {version}"));
    }
    // Debug builds may skip notarization to test the flow with local builds.
    let skip = cfg!(debug_assertions) && std::env::var_os("SHAFT_UPDATE_SKIP_NOTARIZATION").is_some();
    if !skip {
        run_ok("/usr/sbin/spctl", &["--assess", "--type", "execute", &s]).map_err(|e| format!("the new version is not notarized ({e})"))?;
    }
    Ok(())
}

/// Swaps the bundles once `pid` has exited, then relaunches. If the swap
/// fails the old bundle is restored, so Shaft always comes back.
const SWAP_SCRIPT: &str = r#"#!/bin/sh
pid="$1"; old="$2"; new="$3"; backup="$old.previous"
i=0
while kill -0 "$pid" 2>/dev/null; do
  sleep 0.2; i=$((i + 1))
  if [ "$i" -gt 300 ]; then echo "Shaft did not quit; update cancelled"; rm -rf "$new"; exit 1; fi
done
rm -rf "$backup"
if mv "$old" "$backup" && mv "$new" "$old"; then
  rm -rf "$backup"
  echo "updated $old"
else
  echo "swap failed; restoring"
  [ -d "$old" ] || mv "$backup" "$old"
  rm -rf "$new"
fi
exec /usr/bin/open "$old"
"#;

/// Start the helper that replaces `target.bundle` with `staged` after this
/// process exits. The caller must quit right after.
pub fn spawn_swap(target: &Target, staged: &Path, log: &Path) -> Result<(), String> {
    let dir = staged.parent().ok_or("invalid staging path")?;
    let script = std::env::temp_dir().join(format!("shaft-update-{}.sh", std::process::id()));
    std::fs::write(&script, SWAP_SCRIPT).map_err(|e| format!("could not write the update helper: {e}"))?;
    let log_file = std::fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| e.to_string())?;
    Command::new("/bin/sh")
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(&target.bundle)
        .arg(staged)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(log_file.try_clone().map_err(|e| e.to_string())?)
        .stderr(log_file)
        // Own process group: not killed along with Shaft.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start the update helper: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_team_id() {
        let out =
            "Executable=/x\nIdentifier=dev.shaft.app\nAuthority=Developer ID Application: X (ABCDE12345)\nTeamIdentifier=ABCDE12345\n";
        assert_eq!(parse_team_id(out).as_deref(), Some("ABCDE12345"));
        assert_eq!(parse_team_id("Signature=adhoc\nTeamIdentifier=not set\n"), None);
        assert_eq!(parse_team_id("Signature=adhoc\n"), None);
    }

    #[test]
    fn parses_checksum() {
        let h = "efad317fbf06f328d8d82d3a2d41be225066e01019ac546f388e4ec750af0821";
        assert_eq!(parse_checksum(&format!("{h}  Shaft-0.2.0.dmg\n")).as_deref(), Some(h));
        assert_eq!(parse_checksum(&h.to_uppercase()).as_deref(), Some(h));
        assert_eq!(parse_checksum("nothex  file"), None);
        assert_eq!(parse_checksum(""), None);
    }
}
