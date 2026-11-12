//! In-memory ring buffer of captured HTTP exchanges.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::watch;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct CapturedBody {
    /// The first `max_body_bytes` of the body (raw, still content-encoded).
    #[serde(skip)]
    pub bytes: Bytes,
    /// Total bytes seen on the wire.
    pub total: u64,
    /// `bytes` holds less than `total`.
    pub truncated: bool,
    /// The body stream finished (false while in flight or if aborted).
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExchangeState {
    /// Waiting for the upstream response.
    Pending,
    /// Response headers received, body streaming.
    Streaming,
    Complete,
    /// Protocol upgrade (e.g. WebSocket); frames are not captured.
    Upgraded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Exchange {
    pub id: u64,
    pub started: SystemTime,
    pub duration: Option<Duration>,
    pub remote_addr: Option<SocketAddr>,
    pub method: String,
    /// Path and query as received.
    pub uri: String,
    pub version: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: CapturedBody,
    pub status: Option<u16>,
    pub response_headers: Vec<(String, String)>,
    pub response_body: CapturedBody,
    pub state: ExchangeState,
    pub error: Option<String>,
    /// Id of the exchange this one replays.
    pub replay_of: Option<u64>,
}

impl Exchange {
    pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn request_header(&self, name: &str) -> Option<&str> {
        Self::header(&self.request_headers, name)
    }
    pub fn response_header(&self, name: &str) -> Option<&str> {
        Self::header(&self.response_headers, name)
    }
}

/// Lightweight row for list views.
#[derive(Debug, Clone, PartialEq)]
pub struct ExchangeSummary {
    pub id: u64,
    pub started: SystemTime,
    pub duration: Option<Duration>,
    pub method: String,
    pub uri: String,
    pub status: Option<u16>,
    pub state: ExchangeState,
    pub response_size: u64,
    pub replay_of: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_requests: usize,
    pub max_body_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_requests: 500, max_body_bytes: 1024 * 1024 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub total: u64,
    pub upstream_errors: u64,
}

struct Inner {
    next_id: u64,
    items: VecDeque<Exchange>,
    limits: Limits,
    counters: Counters,
}

/// Thread-safe exchange store. Every mutation bumps a generation counter that
/// observers can watch via [`InspectorStore::subscribe`].
pub struct InspectorStore {
    inner: Mutex<Inner>,
    generation: watch::Sender<u64>,
}

impl InspectorStore {
    pub fn new(limits: Limits) -> Self {
        let (tx, _) = watch::channel(0);
        Self { inner: Mutex::new(Inner { next_id: 1, items: VecDeque::new(), limits, counters: Counters::default() }), generation: tx }
    }

    pub fn limits(&self) -> Limits {
        self.inner.lock().limits
    }

    pub fn set_limits(&self, limits: Limits) {
        let mut g = self.inner.lock();
        g.limits = limits;
        while g.items.len() > limits.max_requests {
            g.items.pop_front();
        }
        drop(g);
        self.bump();
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    fn bump(&self) {
        self.generation.send_modify(|g| *g = g.wrapping_add(1));
    }

    /// Record a new request and return its id.
    pub fn begin(
        &self,
        method: String,
        uri: String,
        version: String,
        request_headers: Vec<(String, String)>,
        remote_addr: Option<SocketAddr>,
        replay_of: Option<u64>,
    ) -> u64 {
        let mut g = self.inner.lock();
        let id = g.next_id;
        g.next_id += 1;
        g.counters.total += 1;
        g.items.push_back(Exchange {
            id,
            started: SystemTime::now(),
            duration: None,
            remote_addr,
            method,
            uri,
            version,
            request_headers,
            request_body: CapturedBody::default(),
            status: None,
            response_headers: Vec::new(),
            response_body: CapturedBody::default(),
            state: ExchangeState::Pending,
            error: None,
            replay_of,
        });
        while g.items.len() > g.limits.max_requests.max(1) {
            g.items.pop_front();
        }
        drop(g);
        self.bump();
        id
    }

    /// Mutate an exchange if it is still in the buffer.
    pub fn update(&self, id: u64, f: impl FnOnce(&mut Exchange)) {
        let mut g = self.inner.lock();
        let mut failed_before = false;
        let mut failed_after = false;
        if let Some(e) = g.items.iter_mut().find(|e| e.id == id) {
            failed_before = e.state == ExchangeState::Failed;
            f(e);
            failed_after = e.state == ExchangeState::Failed;
        }
        if failed_after && !failed_before {
            g.counters.upstream_errors += 1;
        }
        drop(g);
        self.bump();
    }

    pub fn get(&self, id: u64) -> Option<Exchange> {
        self.inner.lock().items.iter().find(|e| e.id == id).cloned()
    }

    /// All exchanges, oldest first (bodies are cheap `Bytes` clones).
    pub fn all(&self) -> Vec<Exchange> {
        self.inner.lock().items.iter().cloned().collect()
    }

    pub fn summaries(&self) -> Vec<ExchangeSummary> {
        self.inner
            .lock()
            .items
            .iter()
            .map(|e| ExchangeSummary {
                id: e.id,
                started: e.started,
                duration: e.duration,
                method: e.method.clone(),
                uri: e.uri.clone(),
                status: e.status,
                state: e.state,
                response_size: e.response_body.total,
                replay_of: e.replay_of,
                error: e.error.clone(),
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn counters(&self) -> Counters {
        self.inner.lock().counters
    }

    pub fn clear(&self) {
        self.inner.lock().items.clear();
        self.bump();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_evicts_oldest() {
        let s = InspectorStore::new(Limits { max_requests: 2, max_body_bytes: 10 });
        let mut rx = s.subscribe();
        let a = s.begin("GET".into(), "/a".into(), "HTTP/1.1".into(), vec![], None, None);
        let _b = s.begin("GET".into(), "/b".into(), "HTTP/1.1".into(), vec![], None, None);
        let c = s.begin("GET".into(), "/c".into(), "HTTP/1.1".into(), vec![], None, None);
        assert_eq!(s.len(), 2);
        assert!(s.get(a).is_none());
        assert_eq!(s.get(c).unwrap().uri, "/c");
        assert_eq!(s.counters().total, 3);
        assert!(rx.has_changed().unwrap());
        rx.mark_unchanged();
        s.update(c, |e| e.state = ExchangeState::Failed);
        s.update(c, |e| e.state = ExchangeState::Failed);
        assert_eq!(s.counters().upstream_errors, 1);
        assert!(rx.has_changed().unwrap());
        s.set_limits(Limits { max_requests: 1, max_body_bytes: 10 });
        assert_eq!(s.summaries().len(), 1);
        s.clear();
        assert!(s.is_empty());
    }
}
