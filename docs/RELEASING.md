# Releasing Shaft

Shaft ships outside the Mac App Store as a **Developer ID–signed, notarized DMG**
on GitHub Releases (plus an optional Homebrew cask). The app finds new versions
by asking GitHub for the latest published release. It does not install them
itself; the user downloads the new DMG or runs `brew upgrade`.

Why not the Mac App Store: it requires the App Sandbox, and it forbids
in-app update checks that point outside the store. Shaft opens arbitrary local
ports and registers global hotkeys, so Developer ID distribution fits it better.

## One-time setup (Apple Developer Program)

1. **Enroll** at <https://developer.apple.com/programs/> (99 USD/year). Note
   your **Team ID** (Membership details, 10 characters).
2. **Create a Developer ID Application certificate.** This is *not* the
   "Apple Development" certificate Xcode creates by default.
   - Xcode → Settings → Accounts → your team → Manage Certificates → `+` →
     **Developer ID Application**. Only the Account Holder can create it.
   - Check: `security find-identity -v -p codesigning` should list
     `Developer ID Application: Your Name (TEAMID)`.
3. **Export it for CI.** In Keychain Access, right-click the certificate (with
   its private key) → Export → `.p12` with a strong password. Keep a backup
   offline: a lost key means a new certificate.
4. **Create an app-specific password for notarization.** Sign in at
   <https://account.apple.com> with an Apple ID that belongs to the team →
   Sign-In and Security → App-Specific Passwords → generate one (e.g.
   "notarytool"). Changing that Apple ID's password revokes it: generate a new
   one and update the `APPLE_APP_PASSWORD` secret.
5. **Store local notarization credentials** (for releases built on your Mac;
   it asks for the app-specific password):

   ```bash
   xcrun notarytool store-credentials shaft --apple-id you@example.com --team-id TEAMID
   ```

6. **Add GitHub secrets** (repository → Settings → Secrets and variables → Actions):

   | Secret | Value |
   |---|---|
   | `MACOS_CERT_P12` | `base64 -i DeveloperID.p12 \| pbcopy` |
   | `MACOS_CERT_PASSWORD` | the `.p12` password |
   | `MACOS_SIGN_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
   | `APPLE_ID` | the Apple ID email |
   | `APPLE_TEAM_ID` | Team ID |
   | `APPLE_APP_PASSWORD` | the app-specific password |

### Alternative: App Store Connect API key

A team API key does not depend on one person's Apple ID. The Account Holder
must first enable the API (App Store Connect → Users and Access → Integrations
→ App Store Connect API → Request Access); then an Admin can create a Team Key
with the **Developer** role and download `AuthKey_XXXX.p8` (only once). Add
these secrets; when `NOTARY_KEY_P8` is set, CI uses the key instead of the
Apple ID, so the `APPLE_*` secrets can then be deleted:

| Secret | Value |
|---|---|
| `NOTARY_KEY_P8` | contents of `AuthKey_XXXX.p8` |
| `NOTARY_KEY_ID` | Key ID |
| `NOTARY_ISSUER` | Issuer ID |

Locally: `xcrun notarytool store-credentials shaft --key AuthKey_XXXX.p8 --key-id … --issuer …`.

## Cutting a release

```bash
scripts/release.sh 0.2.0
```

```bash
git push origin HEAD v0.2.0
```

`release.sh` bumps the workspace version (and the cask), runs the tests,
commits and creates the tag. The tag starts the `release` CI job, which:

1. checks that the tag matches the `Cargo.toml` version,
2. builds a universal (arm64 + x86_64) binary and assembles `Shaft.app`,
3. signs it with the hardened runtime and a secure timestamp,
4. notarizes and staples the app, then builds, signs, notarizes and staples the DMG,
5. publishes a GitHub release with the DMG, its `.sha256` and generated notes.

Shaft finds an update once that release is **published** and not marked as a
pre-release. Tags with a suffix (`v0.3.0-beta.1`) become pre-releases, which
the update check ignores.

Then update the Homebrew cask's `sha256` from `Shaft-0.2.0.dmg.sha256` and push
it to your tap.

## Building a release locally

```bash
UNIVERSAL=1 DMG=1 NOTARY_PROFILE=shaft SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" scripts/bundle.sh
```

Verify what users will get:

```bash
spctl --assess --type execute --verbose=2 dist/Shaft.app
```

```bash
xcrun stapler validate dist/Shaft-*.dmg
```

## How update detection works

- Feed: `https://api.github.com/repos/nnevskij/shaft/releases/latest`. It
  ignores drafts and pre-releases. Versions are compared by SemVer against
  the running build's `Cargo.toml` version.
- Schedule: 15 s after launch, then every 24 h of wall-clock time, checked
  hourly so a Mac that slept still checks on wake. You can turn it off in
  Settings → Software Update, which also has **Check Now**.
- When a newer version exists, the user gets a notification (once per version
  per run) and a banner in the menu bar popover with **Download** (the DMG
  asset, else the release page), **What's New** and **Skip This Version**.
  Skipped versions are not offered again automatically, but manual checks
  still show them.
- Automatic checks only run from the `.app` bundle. To test against a mock feed:

  ```bash
  SHAFT_UPDATE_URL=http://127.0.0.1:8765/latest cargo run -p shaft-app
  ```
