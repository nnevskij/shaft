# Shaft

A native macOS menu bar client for [ngrok](https://ngrok.com), written in Rust.

Shaft lives only in the menu bar: start and stop tunnels, copy their URLs, inspect and replay HTTP traffic, manage several ngrok accounts and the resources on them (domains, endpoints, TCP addresses, credentials, IP policies…) without keeping a terminal open or switching to the dashboard.

> Shaft is an **unofficial** client. ngrok is a trademark of ngrok Inc.

## Features

- **Tunnels** — HTTP(S), TCP and TLS endpoints with every option of `ngrok http|tcp|tls`: custom domains and reserved addresses, basic auth, OAuth, OIDC, webhook verification, IP restrictions, header rules, host rewrite, compression, circuit breaker, mutual TLS, TLS termination, PROXY protocol, bindings, pooling and Traffic Policy (YAML or JSON).
- **Quick tunnels** — type a port, pick a protocol, press Enter. The URL is copied for you.
- **Profiles** — save tunnels you use often; favourites are pinned in the menu and can have a global shortcut.
- **Inspector** — live list of HTTP requests per tunnel with headers and bodies (gzip/brotli decoded, JSON pretty-printed, hex for binary), filters, replay, edit-and-replay, copy as `curl`, HAR export.
- **Accounts** — several ngrok accounts side by side; authtokens and API keys are stored in the macOS Keychain, never on disk.
- **ngrok.yml** — import v2/v3 agent configs, export profiles as v3 (options without a v3 field are translated to Traffic Policy rules).
- **Resources** — list/create/edit/delete reserved domains, TCP addresses, cloud endpoints, agents (restart/update/stop), authtokens, API keys, SSH keys, IP policies & rules, IP restrictions, TLS certificates, event subscriptions/destinations, plus a generic API explorer for everything else.
- **Diagnostics** — DNS/TCP/TLS checks to the ngrok edge and authtoken/API key validation, with a shareable report.
- **Resilience** — automatic reconnection after sleep or network changes, upstream reachability probe, notifications on state changes.

No ngrok binary is required: tunnels run in-process through the official [ngrok-rust SDK](https://github.com/ngrok/ngrok-rust); account resources use the [ngrok API](https://ngrok.com/docs/api/).

## Requirements

- macOS 13 Ventura or later (Apple Silicon or Intel)
- An ngrok account ([authtoken](https://dashboard.ngrok.com/get-started/your-authtoken); an API key is only needed for the Resources window)

## Build and run

```bash
cargo run -p shaft-app
```

To build a proper `.app` (needed for notifications):

```bash
scripts/bundle.sh
```

```bash
open dist/Shaft.app
```

`UNIVERSAL=1`, `SIGN_IDENTITY="Developer ID Application: …"`, `DMG=1` and `NOTARY_PROFILE=…` produce a signed, notarized universal DMG.

### Development switches

| Variable | Effect |
|---|---|
| `SHAFT_CONFIG=/path/config.toml` | Use another config file |
| `SHAFT_NO_KEYCHAIN=1` | Keep secrets in memory only |
| `SHAFT_LOG=debug` | Log level (logs: `~/Library/Logs/dev.shaft.app`) |
| `SHAFT_DEBUG_OPEN=popover,inspector,…` | Open UI at launch |
| `SHAFT_SNAPSHOT_DIR=/tmp/snaps` | Render shown views to PNG (no screen recording needed) |

## Tests

```bash
cargo test --workspace
```

An end-to-end test opens a real tunnel when an authtoken is provided:

```bash
SHAFT_E2E_AUTHTOKEN=… cargo test -p shaft-core --test manager -- --ignored
```

## Layout

| Crate | Purpose |
|---|---|
| `shaft-config` | Data model, `config.toml` persistence, Keychain secrets, validation, `ngrok.yml` import/export |
| `shaft-inspector` | Capturing HTTP reverse proxy, exchange store, replay, curl/HAR/body formatting |
| `shaft-api` | ngrok REST API client and resource catalog |
| `shaft-core` | Tunnel manager (sessions per account, reconnection, probes), diagnostics — no UI |
| `shaft-app` | AppKit UI via `objc2`: status item, popover, windows, notifications, hotkeys |

The product requirements are in [docs/shaft-prd.md](docs/shaft-prd.md).

## License

MIT OR Apache-2.0
