//! The tunnel manager: an actor on the tokio runtime that owns ngrok sessions
//! (one per account) and tunnels, and reports state changes to the UI through
//! an [`EventSink`].

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use futures::TryStreamExt;
use ngrok::{
    config::ForwarderBuilder,
    prelude::*,
    session::{default_connect, ConnectError, IoStream, Stop},
    tunnel::AcceptError,
    Session,
};
use parking_lot::RwLock;
use shaft_config::{normalize_upstream, probe_addr, traffic_policy_to_json, Account, Protocol, ProtocolOptions, Settings, TunnelProfile};
use shaft_inspector::{InspectorStore, Limits, Proxy, ProxyConfig, ReplayRequest};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use url::Url;
use uuid::Uuid;

use crate::{
    builder,
    model::{CoreError, CoreEvent, SessionState, TunnelSnapshot, TunnelState},
};

pub type EventSink = Arc<dyn Fn(CoreEvent) + Send + Sync>;

const PROBE_INTERVAL: Duration = Duration::from_secs(5);
const INSPECTOR_EVENT_INTERVAL: Duration = Duration::from_millis(250);

/// Captured traffic of a run, kept after the run stops unless
/// `clear_on_stop` is set.
#[derive(Clone)]
pub struct InspectorEntry {
    pub run_id: Uuid,
    pub profile_id: Uuid,
    pub name: String,
    pub public_url: Option<String>,
    pub store: Arc<InspectorStore>,
    pub proxy: Arc<Proxy>,
    pub active: bool,
}

#[derive(Default)]
struct Shared {
    snapshots: RwLock<HashMap<Uuid, TunnelSnapshot>>,
    inspectors: RwLock<HashMap<Uuid, InspectorEntry>>,
    settings: RwLock<Settings>,
}

enum Msg {
    Start { run_id: Uuid, profile: Box<TunnelProfile>, account: Box<Account> },
    Stop(Uuid),
    StopAll,
    StopAccount(Uuid),
    Settings(Box<Settings>),
    // Internal, from tunnel tasks and connectors.
    TunnelOnline { run_id: Uuid, url: String },
    TunnelFailed { run_id: Uuid, error: CoreError },
    TunnelEnded { run_id: Uuid },
    Session { account_id: Uuid, generation: u64, state: SessionState },
    Upstream { run_id: Uuid, reachable: bool },
    Counters { run_id: Uuid, total: u64, errors: u64 },
    Shutdown(oneshot::Sender<()>),
}

/// Cheap, cloneable handle used by the UI thread.
#[derive(Clone)]
pub struct CoreHandle {
    tx: mpsc::UnboundedSender<Msg>,
    shared: Arc<Shared>,
    rt: tokio::runtime::Handle,
}

impl CoreHandle {
    /// Spawn the manager on `rt`.
    pub fn start(rt: tokio::runtime::Handle, settings: Settings, sink: EventSink) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared::default());
        *shared.settings.write() = settings;
        let actor = Actor { rx, tx: tx.clone(), sink, shared: shared.clone(), sessions: HashMap::new(), tunnels: HashMap::new() };
        rt.spawn(actor.run());
        Self { tx, shared, rt }
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.rt
    }

    /// Start a tunnel; returns its run id. The profile and account must carry
    /// their secrets.
    pub fn start_profile(&self, profile: TunnelProfile, account: Account) -> Uuid {
        let run_id = Uuid::new_v4();
        let _ = self.tx.send(Msg::Start { run_id, profile: Box::new(profile), account: Box::new(account) });
        run_id
    }

    pub fn stop(&self, run_id: Uuid) {
        let _ = self.tx.send(Msg::Stop(run_id));
    }

    /// Stop every run of a profile.
    pub fn stop_profile(&self, profile_id: Uuid) {
        for s in self.snapshots() {
            if s.profile_id == profile_id {
                self.stop(s.run_id);
            }
        }
    }

    pub fn stop_all(&self) {
        let _ = self.tx.send(Msg::StopAll);
    }

    pub fn stop_account(&self, account_id: Uuid) {
        let _ = self.tx.send(Msg::StopAccount(account_id));
    }

    pub fn update_settings(&self, settings: Settings) {
        let _ = self.tx.send(Msg::Settings(Box::new(settings)));
    }

    /// Active runs, oldest first.
    pub fn snapshots(&self) -> Vec<TunnelSnapshot> {
        let mut v: Vec<_> = self.shared.snapshots.read().values().cloned().collect();
        v.sort_by_key(|s| s.started_at);
        v
    }

    pub fn running_profile(&self, profile_id: Uuid) -> Option<TunnelSnapshot> {
        self.shared.snapshots.read().values().find(|s| s.profile_id == profile_id).cloned()
    }

    /// Inspector entries (active first, then most recent).
    pub fn inspectors(&self) -> Vec<InspectorEntry> {
        let mut v: Vec<_> = self.shared.inspectors.read().values().cloned().collect();
        v.sort_by_key(|e| (!e.active, std::cmp::Reverse(e.store.counters().total)));
        v
    }

    pub fn inspector(&self, run_id: Uuid) -> Option<InspectorEntry> {
        self.shared.inspectors.read().get(&run_id).cloned()
    }

    pub fn forget_inspector(&self, run_id: Uuid) {
        let mut g = self.shared.inspectors.write();
        if g.get(&run_id).map(|e| !e.active).unwrap_or(false) {
            g.remove(&run_id);
        }
    }

    /// Replay a request through a run's proxy; `done` runs on the tokio runtime.
    pub fn replay(
        &self,
        run_id: Uuid,
        req: ReplayRequest,
        replay_of: Option<u64>,
        done: impl FnOnce(Result<u64, String>) + Send + 'static,
    ) {
        let Some(entry) = self.inspector(run_id) else {
            done(Err("this tunnel's inspector is no longer available".into()));
            return;
        };
        self.rt.spawn(async move {
            done(entry.proxy.replay(req, replay_of).await.map_err(|e| e.to_string()));
        });
    }

    /// Stop everything and wait (bounded) for sessions to close. Safe to call
    /// from a non-runtime thread.
    pub fn shutdown_blocking(&self, timeout: Duration) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Shutdown(tx)).is_err() {
            return;
        }
        let (std_tx, std_rx) = std::sync::mpsc::channel();
        self.rt.spawn(async move {
            let _ = tokio::time::timeout(timeout, rx).await;
            let _ = std_tx.send(());
        });
        let _ = std_rx.recv_timeout(timeout + Duration::from_millis(200));
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// One ngrok session per account, created lazily by the first tunnel.
struct AccountSession {
    account_id: Uuid,
    fingerprint: u64,
    generation: u64,
    slot: AsyncMutex<Option<Session>>,
    /// Set when the session is closed, so its connector stops retrying.
    cancelled: Arc<AtomicBool>,
}

/// Identifies the connection settings of an account (the token is hashed,
/// never stored).
fn fingerprint(a: &Account) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    a.authtoken.expose().hash(&mut h);
    a.server_addr.hash(&mut h);
    a.proxy_url.hash(&mut h);
    a.root_cas.hash(&mut h);
    h.finish()
}

impl AccountSession {
    async fn get_or_connect(&self, account: &Account, settings: &Settings, tx: &mpsc::UnboundedSender<Msg>) -> Result<Session, CoreError> {
        let mut slot = self.slot.lock().await;
        if let Some(s) = slot.as_ref() {
            return Ok(s.clone());
        }
        if account.authtoken.is_empty() {
            return Err(CoreError::new(format!("account `{}` has no authtoken", account.name)));
        }
        let _ = tx.send(Msg::Session { account_id: self.account_id, generation: self.generation, state: SessionState::Connecting });

        let mut b = Session::builder();
        b.authtoken(account.authtoken.expose().to_string()).client_info("shaft", env!("CARGO_PKG_VERSION"), None::<String>);
        let rc = &settings.reconnect;
        let (interval, tolerance) = rc.heartbeat();
        let _ = b.heartbeat_interval(Duration::from_secs(interval));
        let _ = b.heartbeat_tolerance(Duration::from_secs(tolerance));
        if let Some(addr) = account.server_addr.as_deref().filter(|s| !s.is_empty()) {
            b.server_addr(addr).map_err(|e| CoreError::new(format!("invalid server address: {e}")))?;
        }
        if let Some(p) = account.proxy_url.as_deref().filter(|s| !s.is_empty()) {
            let url: Url = p.parse().map_err(|e| CoreError::new(format!("invalid proxy URL: {e}")))?;
            b.proxy_url(url).map_err(|e| CoreError::new(e.to_string()))?;
        }
        if let Some(cas) = account.root_cas.as_deref().filter(|s| !s.is_empty()) {
            b.root_cas(cas).map_err(|e| CoreError::new(format!("root CAs: {e}")))?;
        }

        // Reconnect reporting: the connector is called with `Some(err)` on
        // every reconnect attempt.
        let attempts = Arc::new(AtomicU32::new(0));
        let max = rc.max_attempts.max(1);
        let (account_id, generation) = (self.account_id, self.generation);
        let txc = tx.clone();
        let cancelled_c = self.cancelled.clone();
        b.connector(move |host: String, port: u16, tls: Arc<rustls::ClientConfig>, err: Option<AcceptError>| {
            let attempts = attempts.clone();
            let txc = txc.clone();
            let cancelled = cancelled_c.clone();
            async move {
                let reconnecting = err.is_some();
                if let Some(e) = &err {
                    if cancelled.load(Ordering::SeqCst) {
                        return Err(ConnectError::Canceled);
                    }
                    let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    if n > max {
                        let _ = txc.send(Msg::Session {
                            account_id,
                            generation,
                            state: SessionState::Failed(CoreError::new(format!("gave up after {max} reconnect attempts: {e}"))),
                        });
                        return Err(ConnectError::Canceled);
                    }
                    let _ = txc.send(Msg::Session {
                        account_id,
                        generation,
                        state: SessionState::Reconnecting { attempt: n, error: e.to_string() },
                    });
                }
                let res: Result<Box<dyn IoStream>, ConnectError> = default_connect(host, port, tls, err).await;
                if res.is_ok() && reconnecting {
                    attempts.store(0, Ordering::SeqCst);
                    let _ = txc.send(Msg::Session { account_id, generation, state: SessionState::Connected });
                }
                res
            }
        });
        // Remote "stop agent" from the dashboard/API.
        let txs = tx.clone();
        b.handle_stop_command(move |_req: Stop| {
            let txs = txs.clone();
            async move {
                let _ = txs.send(Msg::StopAccount(account_id));
                Ok(())
            }
        });

        let session = b.connect().await.map_err(|e| connect_error(&e))?;
        let _ = tx.send(Msg::Session { account_id, generation, state: SessionState::Connected });
        *slot = Some(session.clone());
        Ok(session)
    }

    /// Close the session if one was established. Returns whether it was.
    async fn close(&self) -> bool {
        self.cancelled.store(true, Ordering::SeqCst);
        let Some(mut s) = self.slot.lock().await.take() else { return false };
        if let Err(e) = tokio::time::timeout(Duration::from_secs(3), s.close()).await {
            tracing::debug!(error = %e, "session close timed out");
        }
        true
    }
}

fn connect_error(e: &ConnectError) -> CoreError {
    let code = ngrok::Error::error_code(e);
    let msg = ngrok::Error::msg(e);
    let detail = std::error::Error::source(e).map(|s| s.to_string());
    match (code, detail) {
        (Some(c), _) => CoreError::with_code(Some(c), msg),
        (None, Some(d)) if !msg.contains(&d) => CoreError::new(format!("{msg}: {d}")),
        _ => CoreError::new(msg),
    }
}

fn rpc_error(e: &ngrok::session::RpcError) -> CoreError {
    CoreError::with_code(ngrok::Error::error_code(e), ngrok::Error::msg(e))
}

fn accept_error(e: &AcceptError) -> CoreError {
    match e {
        AcceptError::ListenerClosed { message, error_code } => CoreError::with_code(error_code.as_deref(), message.clone()),
        AcceptError::Reconnect(c) => connect_error(c),
        other => CoreError::new(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

struct TunnelEntry {
    account_id: Uuid,
    stop: Option<oneshot::Sender<()>>,
    failed: Option<CoreError>,
}

struct Actor {
    rx: mpsc::UnboundedReceiver<Msg>,
    tx: mpsc::UnboundedSender<Msg>,
    sink: EventSink,
    shared: Arc<Shared>,
    sessions: HashMap<Uuid, Arc<AccountSession>>,
    tunnels: HashMap<Uuid, TunnelEntry>,
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Actor {
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                Msg::Start { run_id, profile, account } => self.start(run_id, *profile, *account),
                Msg::Stop(run_id) => self.stop(run_id),
                Msg::StopAll => {
                    for id in self.tunnels.keys().copied().collect::<Vec<_>>() {
                        self.stop(id);
                    }
                }
                Msg::StopAccount(account_id) => {
                    let ids: Vec<_> = self.tunnels.iter().filter(|(_, t)| t.account_id == account_id).map(|(id, _)| *id).collect();
                    for id in ids {
                        self.stop(id);
                    }
                    (self.sink)(CoreEvent::RemoteStop { account_id });
                }
                Msg::Settings(s) => {
                    let limits = Limits { max_requests: s.inspector.max_requests.max(1), max_body_bytes: s.inspector.max_body_bytes };
                    for e in self.shared.inspectors.read().values() {
                        e.store.set_limits(limits);
                    }
                    *self.shared.settings.write() = *s;
                }
                Msg::TunnelOnline { run_id, url } => self.update(run_id, |s| {
                    s.public_url = Some(url.clone());
                    s.state = TunnelState::Online;
                }),
                Msg::TunnelFailed { run_id, error } => {
                    if let Some(t) = self.tunnels.get_mut(&run_id) {
                        t.failed = Some(error.clone());
                    }
                    self.update(run_id, |s| s.state = TunnelState::Failed(error.clone()));
                }
                Msg::TunnelEnded { run_id } => self.ended(run_id).await,
                Msg::Session { account_id, generation, state } => self.session_state(account_id, generation, state),
                Msg::Upstream { run_id, reachable } => self.update(run_id, |s| s.upstream_reachable = Some(reachable)),
                Msg::Counters { run_id, total, errors } => {
                    self.update(run_id, |s| {
                        s.requests = total;
                        s.upstream_errors = errors;
                    });
                    (self.sink)(CoreEvent::InspectorChanged { run_id });
                }
                Msg::Shutdown(done) => {
                    for id in self.tunnels.keys().copied().collect::<Vec<_>>() {
                        self.stop(id);
                    }
                    let sessions: Vec<_> = self.sessions.drain().map(|(_, s)| s).collect();
                    tokio::spawn(async move {
                        futures::future::join_all(sessions.iter().map(|s| s.close())).await;
                        let _ = done.send(());
                    });
                }
            }
        }
    }

    fn update(&self, run_id: Uuid, f: impl FnOnce(&mut TunnelSnapshot)) {
        let snap = {
            let mut g = self.shared.snapshots.write();
            let Some(s) = g.get_mut(&run_id) else { return };
            let before = s.clone();
            f(s);
            if *s == before {
                return;
            }
            s.clone()
        };
        (self.sink)(CoreEvent::TunnelChanged(snap));
    }

    fn start(&mut self, run_id: Uuid, profile: TunnelProfile, account: Account) {
        let settings = self.shared.settings.read().clone();
        let protocol = profile.protocol();
        let inspect = profile.inspect && protocol == Protocol::Http;
        let snap = TunnelSnapshot {
            run_id,
            profile_id: profile.id,
            name: profile.name.clone(),
            account_id: account.id,
            protocol,
            upstream: profile.upstream.clone(),
            public_url: None,
            state: TunnelState::Starting,
            upstream_reachable: None,
            started_at: SystemTime::now(),
            inspect,
            requests: 0,
            upstream_errors: 0,
        };
        self.shared.snapshots.write().insert(run_id, snap.clone());
        (self.sink)(CoreEvent::TunnelChanged(snap));

        // Reuse the account's session unless its connection settings changed
        // and nothing else uses it.
        let fp = fingerprint(&account);
        let in_use = self.tunnels.values().any(|t| t.account_id == account.id);
        let session = match self.sessions.get(&account.id) {
            Some(s) if s.fingerprint == fp || in_use => s.clone(),
            existing => {
                if let Some(old) = existing.cloned() {
                    tokio::spawn(async move { old.close().await });
                }
                let s = Arc::new(AccountSession {
                    account_id: account.id,
                    fingerprint: fp,
                    generation: GENERATION.fetch_add(1, Ordering::SeqCst),
                    slot: AsyncMutex::new(None),
                    cancelled: Arc::new(AtomicBool::new(false)),
                });
                self.sessions.insert(account.id, s.clone());
                s
            }
        };

        let (stop_tx, stop_rx) = oneshot::channel();
        self.tunnels.insert(run_id, TunnelEntry { account_id: account.id, stop: Some(stop_tx), failed: None });
        let ctx = TunnelCtx { run_id, profile, account, session, settings, tx: self.tx.clone(), shared: self.shared.clone() };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            // Run in a nested task so a panic is reported instead of lost.
            let res = tokio::spawn(run_tunnel(ctx, stop_rx)).await;
            if let Err(e) = res {
                let _ = tx.send(Msg::TunnelFailed { run_id, error: CoreError::new(format!("internal error: {e}")) });
            }
            let _ = tx.send(Msg::TunnelEnded { run_id });
        });
    }

    fn stop(&mut self, run_id: Uuid) {
        if let Some(t) = self.tunnels.get_mut(&run_id) {
            if let Some(stop) = t.stop.take() {
                let _ = stop.send(());
                self.update(run_id, |s| s.state = TunnelState::Stopping);
            }
        }
    }

    async fn ended(&mut self, run_id: Uuid) {
        let Some(entry) = self.tunnels.remove(&run_id) else { return };
        let snap = self.shared.snapshots.write().remove(&run_id);
        let clear = self.shared.settings.read().inspector.clear_on_stop;
        {
            let mut g = self.shared.inspectors.write();
            if clear {
                g.remove(&run_id);
            } else if let Some(e) = g.get_mut(&run_id) {
                e.active = false;
            }
        }
        if let Some(s) = snap {
            let failed = entry.failed.clone().or_else(|| match &s.state {
                TunnelState::Failed(e) => Some(e.clone()),
                _ => None,
            });
            (self.sink)(CoreEvent::TunnelRemoved { run_id, profile_id: s.profile_id, name: s.name, failed });
        }
        // Close the account session when its last tunnel is gone.
        let account_id = entry.account_id;
        if !self.tunnels.values().any(|t| t.account_id == account_id) {
            if let Some(sess) = self.sessions.remove(&account_id) {
                let sink = self.sink.clone();
                tokio::spawn(async move {
                    if sess.close().await {
                        sink(CoreEvent::SessionChanged { account_id, state: SessionState::Closed });
                    }
                });
            }
        }
    }

    fn session_state(&mut self, account_id: Uuid, generation: u64, state: SessionState) {
        // Ignore stale reports from a replaced session.
        if self.sessions.get(&account_id).map(|s| s.generation) != Some(generation) {
            return;
        }
        let runs: Vec<Uuid> = self.tunnels.iter().filter(|(_, t)| t.account_id == account_id).map(|(id, _)| *id).collect();
        for run_id in runs {
            match &state {
                SessionState::Reconnecting { attempt, .. } => {
                    let attempt = *attempt;
                    self.update(run_id, |s| {
                        if matches!(s.state, TunnelState::Online | TunnelState::Reconnecting { .. }) {
                            s.state = TunnelState::Reconnecting { attempt };
                        }
                    })
                }
                SessionState::Connected => self.update(run_id, |s| {
                    if matches!(s.state, TunnelState::Reconnecting { .. }) && s.public_url.is_some() {
                        s.state = TunnelState::Online;
                    }
                }),
                SessionState::Failed(e) => {
                    let e = e.clone();
                    if let Some(t) = self.tunnels.get_mut(&run_id) {
                        t.failed = Some(e.clone());
                    }
                    self.update(run_id, |s| s.state = TunnelState::Failed(e));
                }
                SessionState::Connecting | SessionState::Closed => {}
            }
        }
        (self.sink)(CoreEvent::SessionChanged { account_id, state });
    }
}

// ---------------------------------------------------------------------------
// Tunnel task
// ---------------------------------------------------------------------------

struct TunnelCtx {
    run_id: Uuid,
    profile: TunnelProfile,
    account: Account,
    session: Arc<AccountSession>,
    settings: Settings,
    tx: mpsc::UnboundedSender<Msg>,
    shared: Arc<Shared>,
}

async fn run_tunnel(ctx: TunnelCtx, mut stop_rx: oneshot::Receiver<()>) {
    let run_id = ctx.run_id;
    let fail = |error: CoreError| {
        tracing::warn!(%run_id, %error, "tunnel failed");
        let _ = ctx.tx.send(Msg::TunnelFailed { run_id, error });
    };

    let protocol = ctx.profile.protocol();
    let upstream = match normalize_upstream(&ctx.profile.upstream, protocol) {
        Ok(u) => u,
        Err(e) => return fail(CoreError::new(format!("upstream: {e}"))),
    };
    let policy = match traffic_policy_to_json(&ctx.profile.traffic_policy) {
        Ok(p) => p,
        Err(e) => return fail(CoreError::new(format!("traffic policy: {e}"))),
    };

    let session = tokio::select! {
        _ = &mut stop_rx => return,
        s = ctx.session.get_or_connect(&ctx.account, &ctx.settings, &ctx.tx) => match s {
            Ok(s) => s,
            Err(e) => return fail(e),
        },
    };

    // Upstream reachability probe.
    let probe = probe_addr(&upstream).map(|(host, port)| {
        let tx = ctx.tx.clone();
        tokio::spawn(async move {
            let mut last = None;
            loop {
                let ok = matches!(
                    tokio::time::timeout(Duration::from_secs(1), tokio::net::TcpStream::connect((host.as_str(), port))).await,
                    Ok(Ok(_))
                );
                if last != Some(ok) {
                    last = Some(ok);
                    let _ = tx.send(Msg::Upstream { run_id, reachable: ok });
                }
                tokio::time::sleep(PROBE_INTERVAL).await;
            }
        })
    });

    let inspect = ctx.profile.inspect && protocol == Protocol::Http && matches!(upstream.scheme(), "http" | "https");
    let result = match &ctx.profile.options {
        ProtocolOptions::Http(h) => {
            let mut b = session.http_endpoint();
            match builder::apply_http(&mut b, &ctx.profile, h, policy, &upstream) {
                Err(e) => Err(e),
                Ok(()) if inspect => {
                    run_inspected(&ctx, b, &upstream, h.host_header_rewrite, h.app_protocol.as_deref() == Some("http2"), &mut stop_rx).await
                }
                Ok(()) => run_forwarded(&ctx, b, upstream.clone(), &mut stop_rx).await,
            }
        }
        ProtocolOptions::Tcp(_) => {
            let mut b = session.tcp_endpoint();
            match builder::apply_tcp(&mut b, &ctx.profile, policy, &upstream) {
                Err(e) => Err(e),
                Ok(()) => run_forwarded(&ctx, b, upstream.clone(), &mut stop_rx).await,
            }
        }
        ProtocolOptions::Tls(t) => {
            let mut b = session.tls_endpoint();
            match builder::apply_tls(&mut b, &ctx.profile, t, policy, &upstream) {
                Err(e) => Err(e),
                Ok(()) => run_forwarded(&ctx, b, upstream.clone(), &mut stop_rx).await,
            }
        }
    };
    if let Some(p) = probe {
        p.abort();
    }
    if let Err(e) = result {
        fail(e);
    }
}

async fn run_forwarded<B>(ctx: &TunnelCtx, b: B, upstream: Url, stop_rx: &mut oneshot::Receiver<()>) -> Result<(), CoreError>
where
    B: ForwarderBuilder,
    ngrok::forwarder::Forwarder<B::Tunnel>: EndpointInfo + TunnelCloser,
{
    let mut fwd = tokio::select! {
        _ = &mut *stop_rx => return Ok(()),
        r = b.listen_and_forward(upstream) => r.map_err(|e| rpc_error(&e))?,
    };
    let _ = ctx.tx.send(Msg::TunnelOnline { run_id: ctx.run_id, url: fwd.url().to_string() });
    tokio::select! {
        _ = &mut *stop_rx => {
            let _ = tokio::time::timeout(Duration::from_secs(3), fwd.close()).await;
            Ok(())
        }
        res = fwd.join() => match res {
            Ok(Ok(())) => Err(CoreError::new("tunnel closed by ngrok")),
            Ok(Err(e)) => Err(CoreError::new(format!("forwarding stopped: {e}"))),
            Err(e) => Err(CoreError::new(format!("forwarder crashed: {e}"))),
        },
    }
}

async fn run_inspected(
    ctx: &TunnelCtx,
    b: ngrok::config::HttpTunnelBuilder,
    upstream: &Url,
    host_rewrite: bool,
    http2: bool,
    stop_rx: &mut oneshot::Receiver<()>,
) -> Result<(), CoreError> {
    let limits = Limits { max_requests: ctx.settings.inspector.max_requests.max(1), max_body_bytes: ctx.settings.inspector.max_body_bytes };
    let store = Arc::new(InspectorStore::new(limits));
    let proxy = Proxy::new(
        ProxyConfig {
            upstream: upstream.clone(),
            host_rewrite,
            verify_upstream_tls: ctx.profile.verify_upstream_tls,
            upstream_http2: http2,
        },
        store.clone(),
    )
    .map_err(|e| CoreError::new(e.to_string()))?;

    let mut tun = tokio::select! {
        _ = &mut *stop_rx => return Ok(()),
        r = b.listen() => r.map_err(|e| rpc_error(&e))?,
    };
    let url = tun.url().to_string();
    ctx.shared.inspectors.write().insert(
        ctx.run_id,
        InspectorEntry {
            run_id: ctx.run_id,
            profile_id: ctx.profile.id,
            name: ctx.profile.name.clone(),
            public_url: Some(url.clone()),
            store: store.clone(),
            proxy: proxy.clone(),
            active: true,
        },
    );
    let _ = ctx.tx.send(Msg::TunnelOnline { run_id: ctx.run_id, url });

    // Forward store changes as throttled events.
    let watcher = {
        let mut rx = store.subscribe();
        let store = store.clone();
        let tx = ctx.tx.clone();
        let run_id = ctx.run_id;
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let c = store.counters();
                let _ = tx.send(Msg::Counters { run_id, total: c.total, errors: c.upstream_errors });
                tokio::time::sleep(INSPECTOR_EVENT_INTERVAL).await;
            }
        })
    };

    let res = loop {
        tokio::select! {
            _ = &mut *stop_rx => {
                let _ = tokio::time::timeout(Duration::from_secs(3), tun.close()).await;
                break Ok(());
            }
            next = tun.try_next() => match next {
                Ok(Some(conn)) => {
                    let remote = conn.remote_addr();
                    tokio::spawn(proxy.clone().serve_connection(conn, Some(remote)));
                }
                Ok(None) => break Err(CoreError::new("tunnel closed by ngrok")),
                Err(e) => break Err(accept_error(&e)),
            },
        }
    };
    watcher.abort();
    res
}
