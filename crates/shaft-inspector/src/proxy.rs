//! Capturing reverse proxy: serves HTTP on a connection accepted from an ngrok
//! tunnel, forwards each request to the upstream and records it in an
//! [`InspectorStore`].

use std::{
    convert::Infallible,
    error::Error as StdError,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use bytes::{Bytes, BytesMut};
use http::{header, HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Uri, Version};
use http_body::{Body, Frame, SizeHint};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use url::Url;

use crate::store::{CapturedBody, ExchangeState, InspectorStore};

pub type BoxError = Box<dyn StdError + Send + Sync>;
pub type ProxyBody = BoxBody<Bytes, BoxError>;

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// `http://` or `https://` upstream base URL.
    pub upstream: Url,
    /// Replace the Host header with the upstream authority.
    pub host_rewrite: bool,
    /// Verify the upstream certificate for https upstreams.
    pub verify_upstream_tls: bool,
    /// Speak HTTP/2 to the upstream (h2c for http, ALPN h2 for https).
    pub upstream_http2: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("unsupported upstream scheme `{0}` (only http and https can be inspected)")]
    Scheme(String),
    #[error("tls setup failed: {0}")]
    Tls(String),
    #[error("invalid request: {0}")]
    Request(String),
    #[error("upstream error: {0}")]
    Upstream(String),
}

/// Hop-by-hop headers that must not be forwarded (RFC 9110 §7.6.1).
const HOP_BY_HOP: [&str; 8] =
    ["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"];

pub struct Proxy {
    cfg: ProxyConfig,
    client: Client<HttpsConnector<HttpConnector>, ProxyBody>,
    store: Arc<InspectorStore>,
}

impl Proxy {
    pub fn new(cfg: ProxyConfig, store: Arc<InspectorStore>) -> Result<Arc<Self>, ProxyError> {
        match cfg.upstream.scheme() {
            "http" | "https" => {}
            other => return Err(ProxyError::Scheme(other.into())),
        }
        let tls = crate::tls::client_config(cfg.verify_upstream_tls).map_err(|e| ProxyError::Tls(e.to_string()))?;
        let builder = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls).https_or_http();
        let connector = if cfg.upstream_http2 { builder.enable_http2().build() } else { builder.enable_http1().build() };
        let client = Client::builder(TokioExecutor::new()).http2_only(cfg.upstream_http2).build(connector);
        Ok(Arc::new(Self { cfg, client, store }))
    }

    pub fn store(&self) -> &Arc<InspectorStore> {
        &self.store
    }

    pub fn config(&self) -> &ProxyConfig {
        &self.cfg
    }

    /// Serve HTTP/1.1 or HTTP/2 on `io` until the client closes it.
    pub async fn serve_connection<IO>(self: Arc<Self>, io: IO, remote: Option<SocketAddr>)
    where
        IO: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
    {
        let this = self.clone();
        let svc = hyper::service::service_fn(move |req| {
            let this = this.clone();
            async move { Ok::<_, Infallible>(this.handle(req, remote).await) }
        });
        if let Err(e) = auto::Builder::new(TokioExecutor::new()).serve_connection_with_upgrades(io, svc).await {
            tracing::debug!(error = %e, "inspector connection ended with error");
        }
    }

    fn upstream_uri(&self, path_and_query: &str) -> Result<Uri, ProxyError> {
        let base = &self.cfg.upstream;
        let authority = match base.port() {
            Some(p) => format!("{}:{p}", base.host_str().unwrap_or("localhost")),
            None => base.host_str().unwrap_or("localhost").to_string(),
        };
        let base_path = base.path().trim_end_matches('/');
        let pq = if path_and_query.starts_with('/') { path_and_query.to_string() } else { format!("/{path_and_query}") };
        format!("{}://{authority}{base_path}{pq}", base.scheme())
            .parse()
            .map_err(|e: http::uri::InvalidUri| ProxyError::Request(e.to_string()))
    }

    fn upstream_host(&self) -> String {
        let base = &self.cfg.upstream;
        match base.port() {
            Some(p) => format!("{}:{p}", base.host_str().unwrap_or("localhost")),
            None => base.host_str().unwrap_or("localhost").to_string(),
        }
    }

    fn forward_headers(&self, src: &HeaderMap, keep_upgrade: bool) -> HeaderMap {
        // Headers named in `Connection` are hop-by-hop too.
        let mut drop: Vec<String> = HOP_BY_HOP.iter().map(|s| s.to_string()).collect();
        for v in src.get_all(header::CONNECTION) {
            if let Ok(v) = v.to_str() {
                drop.extend(v.split(',').map(|s| s.trim().to_ascii_lowercase()));
            }
        }
        let mut out = HeaderMap::with_capacity(src.len());
        for (k, v) in src {
            let name = k.as_str();
            let is_upgrade_hdr = name == "upgrade" || name == "connection";
            if drop.iter().any(|d| d == name) && !(keep_upgrade && is_upgrade_hdr) {
                continue;
            }
            out.append(k.clone(), v.clone());
        }
        if self.cfg.host_rewrite {
            if let Ok(v) = HeaderValue::from_str(&self.upstream_host()) {
                out.insert(header::HOST, v);
            }
        }
        out
    }

    async fn handle(self: Arc<Self>, mut req: Request<Incoming>, remote: Option<SocketAddr>) -> Response<ProxyBody> {
        let started = Instant::now();
        let pq = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
        let mut req_headers = headers_vec(req.headers());
        if req.version() == Version::HTTP_2 {
            if let Some(auth) = req.uri().authority() {
                if !req.headers().contains_key(header::HOST) {
                    req_headers.insert(0, ("host".into(), auth.to_string()));
                }
            }
        }
        let id = self.store.begin(req.method().to_string(), pq.clone(), format!("{:?}", req.version()), req_headers, remote, None);

        let is_upgrade = is_upgrade_request(req.headers());
        let client_upgrade = is_upgrade.then(|| hyper::upgrade::on(&mut req));

        let uri = match self.upstream_uri(&pq) {
            Ok(u) => u,
            Err(e) => return self.fail(id, started, StatusCode::BAD_GATEWAY, e.to_string()),
        };
        let mut headers = self.forward_headers(req.headers(), is_upgrade);
        if req.version() == Version::HTTP_2 && !headers.contains_key(header::HOST) {
            if let Some(auth) = req.uri().authority() {
                if let Ok(v) = HeaderValue::from_str(auth.as_str()) {
                    headers.insert(header::HOST, v);
                }
            }
        }
        let method = req.method().clone();
        let store = self.store.clone();
        let limit = store.limits().max_body_bytes;
        let body = TeeBody::new(req.into_body(), limit, move |captured| {
            store.update(id, |e| e.request_body = captured);
        });

        let mut up = Request::builder().method(method).uri(uri);
        if self.cfg.upstream_http2 {
            up = up.version(Version::HTTP_2);
            headers.remove(header::HOST);
        }
        let mut up_req = match up.body(body.boxed()) {
            Ok(r) => r,
            Err(e) => return self.fail(id, started, StatusCode::BAD_GATEWAY, e.to_string()),
        };
        *up_req.headers_mut() = headers;

        let resp = match self.client.request(up_req).await {
            Ok(r) => r,
            Err(e) => {
                let msg = error_chain(&e);
                return self.fail(id, started, StatusCode::BAD_GATEWAY, format!("upstream unreachable: {msg}"));
            }
        };

        let status = resp.status();
        let resp_headers = headers_vec(resp.headers());
        self.store.update(id, |e| {
            e.status = Some(status.as_u16());
            e.response_headers = resp_headers;
            e.state = ExchangeState::Streaming;
        });

        if status == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(client_upgrade) = client_upgrade {
                let mut resp = resp;
                let upstream_upgrade = hyper::upgrade::on(&mut resp);
                let store = self.store.clone();
                tokio::spawn(async move {
                    match tokio::try_join!(client_upgrade, upstream_upgrade) {
                        Ok((c, u)) => {
                            let mut c = TokioIo::new(c);
                            let mut u = TokioIo::new(u);
                            let _ = tokio::io::copy_bidirectional(&mut c, &mut u).await;
                        }
                        Err(e) => tracing::debug!(error = %e, "upgrade failed"),
                    }
                    store.update(id, |e| e.duration = Some(started.elapsed()));
                });
                self.store.update(id, |e| {
                    e.state = ExchangeState::Upgraded;
                    e.request_body.complete = true;
                    e.response_body.complete = true;
                });
                let (parts, _) = resp.into_parts();
                let mut out = Response::from_parts(parts, empty());
                *out.version_mut() = Version::HTTP_11;
                return out;
            }
        }

        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        let store = self.store.clone();
        let body = TeeBody::new(body, limit, move |captured| {
            let complete = captured.complete;
            store.update(id, |e| {
                e.response_body = captured;
                e.duration = Some(started.elapsed());
                if complete {
                    e.state = ExchangeState::Complete;
                } else {
                    e.state = ExchangeState::Failed;
                    e.error.get_or_insert_with(|| "response body aborted".into());
                }
            });
        });
        Response::from_parts(parts, body.boxed())
    }

    fn fail(&self, id: u64, started: Instant, status: StatusCode, msg: String) -> Response<ProxyBody> {
        tracing::debug!(id, error = %msg, "inspector request failed");
        let body = format!("Shaft: {msg}\n");
        let len = body.len() as u64;
        self.store.update(id, |e| {
            e.state = ExchangeState::Failed;
            e.status = Some(status.as_u16());
            e.error = Some(msg.clone());
            e.duration = Some(started.elapsed());
            e.response_headers = vec![("content-type".into(), "text/plain; charset=utf-8".into())];
            e.response_body = CapturedBody { bytes: Bytes::from(body.clone()), total: len, truncated: false, complete: true };
        });
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(full(Bytes::from(body)))
            .expect("static response")
    }

    /// Re-send a request directly to the upstream and record it as a new
    /// exchange. Returns the new exchange id.
    pub async fn replay(&self, req: ReplayRequest, replay_of: Option<u64>) -> Result<u64, ProxyError> {
        let started = Instant::now();
        let method: http::Method = req.method.parse().map_err(|_| ProxyError::Request(format!("bad method `{}`", req.method)))?;
        let uri = self.upstream_uri(&req.path)?;
        let mut headers = HeaderMap::new();
        for (k, v) in &req.headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|_| ProxyError::Request(format!("bad header name `{k}`")))?;
            let value = HeaderValue::from_str(v).map_err(|_| ProxyError::Request(format!("bad value for `{k}`")))?;
            headers.append(name, value);
        }
        // Body length may have changed while editing.
        headers.remove(header::CONTENT_LENGTH);
        headers.remove(header::TRANSFER_ENCODING);
        let mut headers = self.forward_headers(&headers, false);
        if !req.body.is_empty() || !matches!(method, http::Method::GET | http::Method::HEAD) {
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(req.body.len()));
        }
        let id = self.store.begin(method.to_string(), req.path.clone(), "HTTP/1.1".into(), headers_vec(&headers), None, replay_of);
        let limit = self.store.limits().max_body_bytes;
        self.store.update(id, |e| e.request_body = capture_full(&req.body, limit));

        let mut builder = Request::builder().method(method).uri(uri);
        if self.cfg.upstream_http2 {
            builder = builder.version(Version::HTTP_2);
            headers.remove(header::HOST);
        }
        let mut up_req = builder.body(full(req.body.clone())).map_err(|e| ProxyError::Request(e.to_string()))?;
        *up_req.headers_mut() = headers;

        match self.client.request(up_req).await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let rh = headers_vec(resp.headers());
                let collected = collect_limited(resp.into_body(), limit).await;
                self.store.update(id, |e| {
                    e.status = Some(status);
                    e.response_headers = rh;
                    e.duration = Some(started.elapsed());
                    match collected {
                        Ok(c) => {
                            e.response_body = c;
                            e.state = ExchangeState::Complete;
                        }
                        Err(err) => {
                            e.state = ExchangeState::Failed;
                            e.error = Some(err);
                        }
                    }
                });
                Ok(id)
            }
            Err(e) => {
                let msg = format!("upstream unreachable: {}", error_chain(&e));
                self.store.update(id, |ex| {
                    ex.state = ExchangeState::Failed;
                    ex.error = Some(msg.clone());
                    ex.duration = Some(started.elapsed());
                });
                Err(ProxyError::Upstream(msg))
            }
        }
    }
}

/// A request to replay, possibly edited by the user.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl ReplayRequest {
    /// Build from a captured exchange. Fails if the body was truncated.
    pub fn from_exchange(e: &crate::store::Exchange) -> Result<Self, String> {
        if e.request_body.truncated {
            return Err(format!(
                "request body was truncated at {} of {} bytes and cannot be replayed exactly",
                e.request_body.bytes.len(),
                e.request_body.total
            ));
        }
        Ok(Self { method: e.method.clone(), path: e.uri.clone(), headers: e.request_headers.clone(), body: e.request_body.bytes.clone() })
    }
}

fn is_upgrade_request(h: &HeaderMap) -> bool {
    h.contains_key(header::UPGRADE)
        && h.get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")))
}

fn strip_hop_by_hop(h: &mut HeaderMap) {
    for name in HOP_BY_HOP {
        h.remove(name);
    }
}

pub(crate) fn headers_vec(h: &HeaderMap) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect()
}

fn error_chain(e: &dyn StdError) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        let cs = c.to_string();
        if !s.contains(&cs) {
            s.push_str(": ");
            s.push_str(&cs);
        }
        cur = c.source();
    }
    s
}

fn full(b: Bytes) -> ProxyBody {
    Full::new(b).map_err(|never: Infallible| match never {}).boxed()
}

fn empty() -> ProxyBody {
    full(Bytes::new())
}

fn capture_full(b: &Bytes, limit: usize) -> CapturedBody {
    CapturedBody { bytes: b.slice(..b.len().min(limit)), total: b.len() as u64, truncated: b.len() > limit, complete: true }
}

async fn collect_limited<B>(mut body: B, limit: usize) -> Result<CapturedBody, String>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let mut buf = BytesMut::new();
    let mut total = 0u64;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| e.to_string())?;
        if let Some(d) = frame.data_ref() {
            total += d.len() as u64;
            let room = limit.saturating_sub(buf.len());
            buf.extend_from_slice(&d[..d.len().min(room)]);
        }
    }
    Ok(CapturedBody { truncated: total > buf.len() as u64, bytes: buf.freeze(), total, complete: true })
}

/// Body wrapper that forwards frames untouched while copying the first
/// `limit` bytes; `on_done` fires exactly once when the stream ends, errors
/// or is dropped.
pub(crate) struct TeeBody<B: Body> {
    inner: B,
    buf: BytesMut,
    total: u64,
    limit: usize,
    on_done: Option<Box<dyn FnOnce(CapturedBody) + Send + Sync>>,
}

impl<B: Body> TeeBody<B> {
    pub(crate) fn new(inner: B, limit: usize, on_done: impl FnOnce(CapturedBody) + Send + Sync + 'static) -> Self {
        Self { inner, buf: BytesMut::new(), total: 0, limit, on_done: Some(Box::new(on_done)) }
    }

    fn finish(&mut self, complete: bool) {
        if let Some(f) = self.on_done.take() {
            let bytes = std::mem::take(&mut self.buf).freeze();
            f(CapturedBody { truncated: self.total > bytes.len() as u64, bytes, total: self.total, complete });
        }
    }
}

impl<B: Body> Drop for TeeBody<B> {
    fn drop(&mut self) {
        let complete = self.inner.is_end_stream();
        self.finish(complete);
    }
}

impl<B> Body for TeeBody<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(d) = frame.data_ref() {
                    this.total += d.len() as u64;
                    let room = this.limit.saturating_sub(this.buf.len());
                    if room > 0 {
                        this.buf.extend_from_slice(&d[..d.len().min(room)]);
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => {
                this.finish(false);
                Poll::Ready(Some(Err(e.into())))
            }
            Poll::Ready(None) => {
                this.finish(true);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
