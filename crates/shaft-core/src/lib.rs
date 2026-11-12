//! Shaft core: manages ngrok sessions and tunnels on a tokio runtime and
//! reports state to the UI. Contains no UI code.

pub mod builder;
pub mod diagnostics;
pub mod manager;
pub mod model;

pub use manager::{CoreHandle, EventSink, InspectorEntry};
pub use model::{CoreError, CoreEvent, SessionState, TunnelSnapshot, TunnelState};
