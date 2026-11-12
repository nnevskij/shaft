//! Shaft request inspector: a capturing HTTP reverse proxy that sits between
//! an ngrok tunnel and the local upstream, an in-memory exchange store,
//! replay, and formatting/export helpers.

pub mod format;
pub mod proxy;
pub mod store;
pub mod tls;

pub use proxy::{Proxy, ProxyConfig, ProxyError, ReplayRequest};
pub use store::{CapturedBody, Counters, Exchange, ExchangeState, ExchangeSummary, InspectorStore, Limits};
