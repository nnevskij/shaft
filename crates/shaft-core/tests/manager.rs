//! Tunnel manager behaviour. Offline tests always run; the online test needs
//! `SHAFT_E2E_AUTHTOKEN` and opens a real (short-lived) ngrok endpoint.

use std::{sync::Arc, time::Duration};

use parking_lot::Mutex;
use shaft_config::{Account, Protocol, Settings, TunnelProfile};
use shaft_core::{CoreEvent, CoreHandle, TunnelState};
use tokio::sync::mpsc;

fn setup() -> (CoreHandle, mpsc::UnboundedReceiver<CoreEvent>, Arc<Mutex<Vec<CoreEvent>>>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let sink = Arc::new(move |e: CoreEvent| {
        log2.lock().push(e.clone());
        let _ = tx.send(e);
    });
    let core = CoreHandle::start(tokio::runtime::Handle::current(), Settings::default(), sink);
    (core, rx, log)
}

async fn wait_for(rx: &mut mpsc::UnboundedReceiver<CoreEvent>, secs: u64, mut f: impl FnMut(&CoreEvent) -> bool) -> CoreEvent {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            let e = rx.recv().await.expect("event channel closed");
            if f(&e) {
                return e;
            }
        }
    })
    .await
    .expect("timed out waiting for event")
}

#[tokio::test]
async fn missing_authtoken_fails_and_is_removed() {
    let (core, mut rx, _) = setup();
    let acc = Account::new("empty");
    let p = TunnelProfile::new("web", acc.id, Protocol::Http, "3000");
    let run = core.start_profile(p, acc);

    let e =
        wait_for(&mut rx, 5, |e| matches!(e, CoreEvent::TunnelChanged(s) if s.run_id == run && matches!(s.state, TunnelState::Failed(_))))
            .await;
    let CoreEvent::TunnelChanged(s) = e else { unreachable!() };
    let TunnelState::Failed(err) = s.state else { unreachable!() };
    assert!(err.message.contains("no authtoken"), "{err}");

    let removed = wait_for(&mut rx, 5, |e| matches!(e, CoreEvent::TunnelRemoved { run_id, .. } if *run_id == run)).await;
    let CoreEvent::TunnelRemoved { failed, .. } = removed else { unreachable!() };
    assert!(failed.is_some());
    assert!(core.snapshots().is_empty());
}

#[tokio::test]
async fn invalid_upstream_fails_before_connecting() {
    let (core, mut rx, log) = setup();
    let mut acc = Account::new("a");
    acc.authtoken = "not-used".into();
    let p = TunnelProfile::new("bad", acc.id, Protocol::Tcp, "http://x:1");
    let run = core.start_profile(p, acc);
    wait_for(&mut rx, 5, |e| matches!(e, CoreEvent::TunnelRemoved { run_id, .. } if *run_id == run)).await;
    let failed = log.lock().iter().any(
        |e| matches!(e, CoreEvent::TunnelChanged(s) if matches!(&s.state, TunnelState::Failed(err) if err.message.starts_with("upstream"))),
    );
    assert!(failed);
    // No session was attempted.
    assert!(!log.lock().iter().any(|e| matches!(e, CoreEvent::SessionChanged { .. })));
}

#[tokio::test]
async fn stop_while_connecting() {
    let (core, mut rx, _) = setup();
    let mut acc = Account::new("unreachable");
    acc.authtoken = "x".into();
    // Blackhole address: connect hangs until we stop.
    acc.server_addr = Some("10.255.255.1:443".into());
    let p = TunnelProfile::new("web", acc.id, Protocol::Http, "3000");
    let run = core.start_profile(p, acc);
    wait_for(&mut rx, 5, |e| matches!(e, CoreEvent::TunnelChanged(s) if s.run_id == run)).await;
    core.stop(run);
    let e = wait_for(&mut rx, 5, |e| matches!(e, CoreEvent::TunnelRemoved { run_id, .. } if *run_id == run)).await;
    let CoreEvent::TunnelRemoved { failed, .. } = e else { unreachable!() };
    assert!(failed.is_none(), "user stop is not a failure: {failed:?}");
    core.shutdown_blocking(Duration::from_secs(2));
}

#[tokio::test]
#[ignore = "needs SHAFT_E2E_AUTHTOKEN and network"]
async fn online_inspected_http_tunnel() {
    use http_body_util_compat::*;
    let token = std::env::var("SHAFT_E2E_AUTHTOKEN").expect("SHAFT_E2E_AUTHTOKEN");

    // Local upstream.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(serve_hello(l));

    let (core, mut rx, _) = setup();
    let mut acc = Account::new("e2e");
    acc.authtoken = token.into();
    let p = TunnelProfile::new("e2e", acc.id, Protocol::Http, port.to_string());
    let run = core.start_profile(p, acc);
    let e = wait_for(&mut rx, 30, |e| matches!(e, CoreEvent::TunnelChanged(s) if s.run_id == run && s.state.is_online())).await;
    let CoreEvent::TunnelChanged(s) = e else { unreachable!() };
    let url = s.public_url.unwrap();
    assert!(url.starts_with("https://"));

    let body = fetch(&format!("{url}/hello?x=1")).await;
    assert_eq!(body, "hello from upstream");

    let entry = core.inspector(run).expect("inspector entry");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let ex = entry.store.all();
    assert!(ex.iter().any(|e| e.uri == "/hello?x=1" && e.status == Some(200)), "{ex:?}");

    core.stop(run);
    wait_for(&mut rx, 10, |e| matches!(e, CoreEvent::TunnelRemoved { run_id, .. } if *run_id == run)).await;
    core.shutdown_blocking(Duration::from_secs(3));
}

mod http_body_util_compat {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub async fn serve_hello(l: tokio::net::TcpListener) {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
                let body = "hello from upstream";
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    }

    /// Minimal HTTPS GET using the system `curl` (keeps test deps small).
    pub async fn fetch(url: &str) -> String {
        let out = tokio::process::Command::new("curl")
            .args(["-sS", "--max-time", "20", "-H", "ngrok-skip-browser-warning: 1", url])
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}
