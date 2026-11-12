//! Client behaviour against a local mock of the ngrok API.

use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{body::Incoming, Request, Response};
use hyper_util::rt::TokioIo;
use shaft_api::{resource, ApiClient};
use tokio::net::TcpListener;

async fn mock(hits: Arc<AtomicUsize>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let base = format!("http://{addr}");
    let base2 = base.clone();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let hits = hits.clone();
            let base = base2.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                    let hits = hits.clone();
                    let base = base.clone();
                    async move {
                        let auth = req.headers().get("authorization").map(|v| v.to_str().unwrap().to_string());
                        let ver = req.headers().get("ngrok-version").map(|v| v.to_str().unwrap().to_string());
                        let pq = req.uri().path_and_query().unwrap().as_str().to_string();
                        let resp = |status: u16, body: String| {
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(status)
                                    .header("content-type", "application/json")
                                    .body(Full::new(Bytes::from(body)))
                                    .unwrap(),
                            )
                        };
                        if auth.as_deref() != Some("Bearer k") || ver.as_deref() != Some("2") {
                            return resp(401, r#"{"error_code":"ERR_NGROK_201","msg":"bad key"}"#.into());
                        }
                        match (req.method().as_str(), pq.as_str()) {
                            ("GET", "/reserved_domains") => resp(
                                200,
                                format!(
                                    r#"{{"reserved_domains":[{{"id":"rd_1","domain":"a.ngrok.app"}}],"next_page_uri":"{base}/reserved_domains?before_id=rd_1"}}"#
                                ),
                            ),
                            ("GET", "/reserved_domains?before_id=rd_1") => {
                                resp(200, r#"{"reserved_domains":[{"id":"rd_2","domain":"b.ngrok.app"}],"next_page_uri":null}"#.into())
                            }
                            ("GET", "/limited") => {
                                if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                                    Ok(Response::builder().status(429).header("retry-after", "0").body(Full::new(Bytes::new())).unwrap())
                                } else {
                                    resp(200, r#"{"ok":true}"#.into())
                                }
                            }
                            ("DELETE", "/reserved_domains/rd_1") => {
                                Ok(Response::builder().status(204).body(Full::new(Bytes::new())).unwrap())
                            }
                            ("POST", "/tunnel_sessions/ts_1/restart") => {
                                Ok(Response::builder().status(204).body(Full::new(Bytes::new())).unwrap())
                            }
                            _ => resp(404, r#"{"error_code":"ERR_NGROK_404","msg":"not found"}"#.into()),
                        }
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(s), svc).await;
            });
        }
    });
    base
}

#[tokio::test]
async fn pagination_retry_and_errors() {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = mock(hits.clone()).await;
    let c = ApiClient::with_base(base.parse().unwrap(), "k");

    let rd = resource("reserved_domains").unwrap();
    let items = c.list_all(rd).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1]["domain"], "b.ngrok.app");

    assert_eq!(c.get("/limited").await.unwrap()["ok"], true);
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    c.delete(rd, "rd_1").await.unwrap();
    c.action(resource("tunnel_sessions").unwrap(), "ts_1", "/restart").await.unwrap();

    let e = c.get("/nope").await.unwrap_err();
    assert_eq!(e.status, Some(404));
    assert_eq!(e.code.as_deref(), Some("ERR_NGROK_404"));

    let bad = ApiClient::with_base(base.parse().unwrap(), "wrong");
    let e = bad.validate().await.unwrap_err();
    assert!(e.is_auth());
}
