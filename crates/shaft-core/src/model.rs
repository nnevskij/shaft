//! Types exchanged between the core and the UI.

use std::{fmt, time::SystemTime};

use shaft_config::Protocol;
use uuid::Uuid;

/// An error surfaced to the user, with the ngrok error code when known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub code: Option<String>,
    pub message: String,
}

impl CoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { code: None, message: message.into() }
    }
    pub fn with_code(code: Option<&str>, message: impl Into<String>) -> Self {
        Self { code: code.map(str::to_string), message: message.into() }
    }
    /// `https://ngrok.com/docs/errors/err_ngrok_105`
    pub fn doc_url(&self) -> Option<String> {
        self.code.as_ref().map(|c| format!("https://ngrok.com/docs/errors/{}", c.to_ascii_lowercase()))
    }
    /// One-line hint for well-known errors.
    pub fn hint(&self) -> Option<&'static str> {
        match self.code.as_deref()? {
            "ERR_NGROK_105" | "ERR_NGROK_106" | "ERR_NGROK_107" => Some("The authtoken is invalid or revoked: check the account settings."),
            "ERR_NGROK_108" => Some("Your plan allows a limited number of simultaneous agent sessions."),
            "ERR_NGROK_120" | "ERR_NGROK_121" => Some("The agent version is too old for this account."),
            "ERR_NGROK_302" | "ERR_NGROK_313" | "ERR_NGROK_314" | "ERR_NGROK_319" => {
                Some("This domain is not reserved on your account (or needs a paid plan).")
            }
            "ERR_NGROK_324" | "ERR_NGROK_334" => Some("This endpoint is already online elsewhere: enable pooling or stop the other agent."),
            "ERR_NGROK_8012" => Some("ngrok could not reach your upstream service."),
            _ => None,
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.code {
            Some(c) => write!(f, "{} ({c})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for CoreError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelState {
    Starting,
    Online,
    Reconnecting { attempt: u32 },
    Failed(CoreError),
    Stopping,
}

impl TunnelState {
    pub fn label(&self) -> &'static str {
        match self {
            TunnelState::Starting => "Starting…",
            TunnelState::Online => "Online",
            TunnelState::Reconnecting { .. } => "Reconnecting…",
            TunnelState::Failed(_) => "Error",
            TunnelState::Stopping => "Stopping…",
        }
    }
    pub fn is_online(&self) -> bool {
        matches!(self, TunnelState::Online)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TunnelSnapshot {
    /// Identifies this run (a profile can run many times over the app's life).
    pub run_id: Uuid,
    /// The saved profile, if started from one (quick tunnels have their own id).
    pub profile_id: Uuid,
    pub name: String,
    pub account_id: Uuid,
    pub protocol: Protocol,
    pub upstream: String,
    pub public_url: Option<String>,
    pub state: TunnelState,
    /// Last TCP probe result; None = not probed yet / unix socket.
    pub upstream_reachable: Option<bool>,
    pub started_at: SystemTime,
    pub inspect: bool,
    pub requests: u64,
    pub upstream_errors: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, error: String },
    Failed(CoreError),
    Closed,
}

#[derive(Debug, Clone)]
pub enum CoreEvent {
    TunnelChanged(TunnelSnapshot),
    /// The tunnel stopped and was removed from the active list.
    TunnelRemoved {
        run_id: Uuid,
        profile_id: Uuid,
        name: String,
        failed: Option<CoreError>,
    },
    SessionChanged {
        account_id: Uuid,
        state: SessionState,
    },
    /// Captured traffic for a run changed (throttled).
    InspectorChanged {
        run_id: Uuid,
    },
    /// The ngrok dashboard/API asked this agent to stop.
    RemoteStop {
        account_id: Uuid,
    },
}
