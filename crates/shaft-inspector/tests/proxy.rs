//! End-to-end proxy tests against a real local upstream.

use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use shaft_inspector::{ExchangeState, InspectorStore, Limits, Proxy, ProxyConfig, ReplayRequest};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// Upstream: echoes method, path, Host and body; `/ws` performs an upgrade
/// and echoes bytes back.
async fn upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(|mut req: Request<Incoming>| async move {
                    if req.uri().path() == "/ws" {
                        let up = hyper::upgrade::on(&mut req);
                        tokio::spawn(async move {
                            let mut io = TokioIo::new(up.await.unwrap());
                            let mut buf = [0u8; 5];
                            io.read_exact(&mut buf).await.unwrap();
                            io.write_all(&buf).await.unwrap();
                        });
                        return Ok::<_, Infallible>(
                            Response::builder()
                                .status(101)
                                .header("connection", "upgrade")
                                .header("upgrade", "echo")
                                .body(Full::new(Bytes::new()))
                                .unwrap(),
                        );
                    }
                    let host = req.headers().get("host").map(|h| h.to_str().unwrap().to_string()).unwrap_or_default();
                    let line = format!("{} {} host={} ", req.method(), req.uri(), host);
                    let body = req.into_body().collect().await.unwrap().to_bytes();
                    let mut out = line.into_bytes();
                    out.extend_from_slice(&body);
                    Ok(Response::builder().header("x-up", "1").body(Full::new(Bytes::from(out))).unwrap())
                });
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(s), svc).with_upgrades().await;
            });
        }
    });
    addr
}

/// Front: accepts TCP and hands each connection to the proxy (stands in for
/// the ngrok tunnel accept loop).
async fn front(proxy: Arc<Proxy>) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, peer) = l.accept().await.unwrap();
            tokio::spawn(proxy.clone().serve_connection(TokioIo::new(s), Some(peer)));
        }
    });
    addr
}

fn client() -> hyper_util::client::legacy::Client<hyper_util::client::legacy::connect::HttpConnector, Full<Bytes>> {
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build_http()
}

async fn wait_complete(store: &InspectorStore, id: u64) -> shaft_inspector::Exchange {
    for _ in 0..100 {
        if let Some(e) = store.get(id) {
            if matches!(e.state, ExchangeState::Complete | ExchangeState::Failed | ExchangeState::Upgraded) {
                return e;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("exchange {id} never completed: {:?}", store.get(id));
}

fn cfg(up: SocketAddr, host_rewrite: bool) -> ProxyConfig {
    ProxyConfig { upstream: format!("http://{up}").parse().unwrap(), host_rewrite, verify_upstream_tls: true, upstream_http2: false }
}

#[tokio::test]
async fn captures_and_replays() {
    let up = upstream().await;
    let store = Arc::new(InspectorStore::new(Limits { max_requests: 10, max_body_bytes: 8 }));
    let proxy = Proxy::new(cfg(up, false), store.clone()).unwrap();
    let addr = front(proxy.clone()).await;

    let req = Request::post(format!("http://{addr}/hook?a=1"))
        .header("host", "demo.ngrok.app")
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from_static(b"hello world")))
        .unwrap();
    let resp = client().request(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-up"], "1");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"POST /hook?a=1 host=demo.ngrok.app hello world");

    let e = wait_complete(&store, 1).await;
    assert_eq!(e.method, "POST");
    assert_eq!(e.uri, "/hook?a=1");
    assert_eq!(e.status, Some(200));
    assert_eq!(e.request_body.total, 11);
    assert!(e.request_body.truncated, "8-byte capture limit");
    assert_eq!(&e.request_body.bytes[..], b"hello wo");
    assert!(e.response_body.complete);
    assert!(e.duration.is_some());
    assert!(e.remote_addr.is_some());

    // Truncated body cannot be replayed verbatim…
    assert!(ReplayRequest::from_exchange(&e).is_err());
    // …but an edited replay works and is linked to the original.
    let id = proxy
        .replay(
            ReplayRequest {
                method: "PUT".into(),
                path: "/edited".into(),
                headers: vec![("host".into(), "demo.ngrok.app".into())],
                body: Bytes::from_static(b"x"),
            },
            Some(e.id),
        )
        .await
        .unwrap();
    let r = store.get(id).unwrap();
    assert_eq!(r.replay_of, Some(1));
    assert_eq!(r.status, Some(200));
    assert_eq!(&r.response_body.bytes[..], b"PUT /edi");
    assert_eq!(store.counters().total, 2);
}

#[tokio::test]
async fn host_rewrite() {
    let up = upstream().await;
    let store = Arc::new(InspectorStore::new(Limits::default()));
    let proxy = Proxy::new(cfg(up, true), store.clone()).unwrap();
    let addr = front(proxy).await;
    let req = Request::get(format!("http://{addr}/")).header("host", "public.example").body(Full::new(Bytes::new())).unwrap();
    let body = client().request(req).await.unwrap().into_body().collect().await.unwrap().to_bytes();
    assert_eq!(String::from_utf8_lossy(&body), format!("GET / host={up} "));
}

#[tokio::test]
async fn upstream_down_is_502() {
    // Bind and drop to get a port that refuses connections.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let store = Arc::new(InspectorStore::new(Limits::default()));
    let proxy = Proxy::new(cfg(dead, false), store.clone()).unwrap();
    let addr = front(proxy).await;
    let resp = client().request(Request::get(format!("http://{addr}/x")).body(Full::new(Bytes::new())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), 502);
    let e = wait_complete(&store, 1).await;
    assert_eq!(e.state, ExchangeState::Failed);
    assert!(e.error.unwrap().contains("upstream unreachable"));
    assert_eq!(store.counters().upstream_errors, 1);
}

#[tokio::test]
async fn upgrade_passthrough() {
    let up = upstream().await;
    let store = Arc::new(InspectorStore::new(Limits::default()));
    let proxy = Proxy::new(cfg(up, false), store.clone()).unwrap();
    let addr = front(proxy).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /ws HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: echo\r\n\r\n").await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    s.write_all(b"ping!").await.unwrap();
    let mut echo = [0u8; 5];
    s.read_exact(&mut echo).await.unwrap();
    assert_eq!(&echo, b"ping!");
    let e = wait_complete(&store, 1).await;
    assert_eq!(e.state, ExchangeState::Upgraded);
    assert_eq!(e.status, Some(101));
}

#[test]
fn rejects_non_http_upstream() {
    let store = Arc::new(InspectorStore::new(Limits::default()));
    let mut c = cfg("127.0.0.1:1".parse().unwrap(), false);
    c.upstream = "tcp://localhost:22".parse().unwrap();
    assert!(Proxy::new(c, store).is_err());
}
