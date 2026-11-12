//! HTTP client for the ngrok API.

use std::time::Duration;

use reqwest::{header, Method, StatusCode};
use serde_json::Value;
use url::Url;

use crate::catalog::ResourceKind;

pub const DEFAULT_BASE: &str = "https://api.ngrok.com";
const API_VERSION: &str = "2";
const MAX_RETRIES: u32 = 3;
const MAX_PAGES: usize = 100;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", self.display())]
pub struct ApiError {
    pub status: Option<u16>,
    /// `ERR_NGROK_…` when the API returned one.
    pub code: Option<String>,
    pub message: String,
}

impl ApiError {
    fn display(&self) -> String {
        let mut s = String::new();
        if let Some(st) = self.status {
            s.push_str(&format!("HTTP {st}"));
        }
        if let Some(c) = &self.code {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(&format!("[{c}]"));
        }
        if !s.is_empty() {
            s.push_str(": ");
        }
        s.push_str(&self.message);
        s
    }

    /// Link to ngrok's error documentation, when there is an error code.
    pub fn doc_url(&self) -> Option<String> {
        self.code.as_ref().map(|c| format!("https://ngrok.com/docs/errors/{}", c.to_ascii_lowercase()))
    }

    pub fn is_auth(&self) -> bool {
        matches!(self.status, Some(401))
    }

    pub fn is_forbidden(&self) -> bool {
        matches!(self.status, Some(403))
    }

    fn transport(e: reqwest::Error) -> Self {
        Self { status: None, code: None, message: format!("network error: {e}") }
    }
}

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: Url,
    key: String,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient").field("base", &self.base.as_str()).finish_non_exhaustive()
    }
}

impl ApiClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base(DEFAULT_BASE.parse().expect("valid default base"), api_key)
    }

    pub fn with_base(base: Url, api_key: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("shaft/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { http, base, key: api_key.into() }
    }

    fn url(&self, path_or_uri: &str) -> Result<Url, ApiError> {
        let res = if path_or_uri.starts_with("http://") || path_or_uri.starts_with("https://") {
            let u: Url = path_or_uri.parse().map_err(|e: url::ParseError| ApiError { status: None, code: None, message: e.to_string() })?;
            // Never send the API key to another host (e.g. a crafted next_page_uri).
            if u.host_str() != self.base.host_str() || u.port_or_known_default() != self.base.port_or_known_default() {
                return Err(ApiError { status: None, code: None, message: format!("refusing to call foreign host in `{path_or_uri}`") });
            }
            u
        } else {
            let p = if path_or_uri.starts_with('/') { path_or_uri.to_string() } else { format!("/{path_or_uri}") };
            self.base.join(&p).map_err(|e| ApiError { status: None, code: None, message: e.to_string() })?
        };
        Ok(res)
    }

    /// Perform a request. `body` is sent as JSON. Retries on 429 (honouring
    /// `Retry-After`) and once on 502/503/504.
    pub async fn request(&self, method: Method, path_or_uri: &str, body: Option<&Value>) -> Result<Value, ApiError> {
        let url = self.url(path_or_uri)?;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut req = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(&self.key)
                .header("Ngrok-Version", API_VERSION)
                .header(header::ACCEPT, "application/json");
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await.map_err(ApiError::transport)?;
            let status = resp.status();
            let retry_after = resp.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
            let text = resp.text().await.map_err(ApiError::transport)?;

            if status == StatusCode::TOO_MANY_REQUESTS && attempt <= MAX_RETRIES {
                let wait = retry_after.unwrap_or(1u64 << (attempt - 1)).min(30);
                tracing::debug!(wait, "ngrok API rate limited, backing off");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            if matches!(status.as_u16(), 502..=504) && attempt == 1 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&text).map_err(|e| ApiError {
                    status: Some(status.as_u16()),
                    code: None,
                    message: format!("invalid JSON in response: {e}"),
                });
            }
            return Err(parse_error(status, &text));
        }
    }

    pub async fn get(&self, path: &str) -> Result<Value, ApiError> {
        self.request(Method::GET, path, None).await
    }

    /// Fetch every page of a collection.
    pub async fn list_all(&self, kind: &ResourceKind) -> Result<Vec<Value>, ApiError> {
        self.list_all_at(kind.path, kind.list_key).await
    }

    pub async fn list_all_at(&self, path: &str, list_key: &str) -> Result<Vec<Value>, ApiError> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        let mut pages = 0;
        while let Some(uri) = next.take() {
            pages += 1;
            if pages > MAX_PAGES {
                tracing::warn!(path, "stopping pagination after {MAX_PAGES} pages");
                break;
            }
            let page = self.get(&uri).await?;
            if let Some(items) = page.get(list_key).and_then(|v| v.as_array()) {
                out.extend(items.iter().cloned());
            }
            next = page.get("next_page_uri").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
        }
        Ok(out)
    }

    pub async fn create(&self, kind: &ResourceKind, body: &Value) -> Result<Value, ApiError> {
        self.request(Method::POST, kind.path, Some(body)).await
    }

    pub async fn update(&self, kind: &ResourceKind, id: &str, body: &Value) -> Result<Value, ApiError> {
        self.request(Method::PATCH, &format!("{}/{}", kind.path, encode(id)), Some(body)).await
    }

    pub async fn delete(&self, kind: &ResourceKind, id: &str) -> Result<(), ApiError> {
        self.request(Method::DELETE, &format!("{}/{}", kind.path, encode(id)), None).await.map(|_| ())
    }

    pub async fn action(&self, kind: &ResourceKind, id: &str, suffix: &str) -> Result<Value, ApiError> {
        self.request(Method::POST, &format!("{}/{}{}", kind.path, encode(id), suffix), Some(&Value::Object(Default::default()))).await
    }

    /// Cheap call to check that the key works.
    pub async fn validate(&self) -> Result<(), ApiError> {
        self.get("/api_keys?limit=1").await.map(|_| ())
    }
}

fn encode(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

fn parse_error(status: StatusCode, text: &str) -> ApiError {
    let v: Option<Value> = serde_json::from_str(text).ok();
    let code = v.as_ref().and_then(|v| v.get("error_code")).and_then(|v| v.as_str()).map(str::to_string);
    let message = v.as_ref().and_then(|v| v.get("msg")).and_then(|v| v.as_str()).map(str::to_string).unwrap_or_else(|| {
        let t = text.trim();
        if t.is_empty() {
            status.canonical_reason().unwrap_or("request failed").to_string()
        } else {
            t.chars().take(300).collect()
        }
    });
    ApiError { status: Some(status.as_u16()), code, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_parsing() {
        let e = parse_error(StatusCode::NOT_FOUND, r#"{"error_code":"ERR_NGROK_404","status_code":404,"msg":"Resource not found"}"#);
        assert_eq!(e.code.as_deref(), Some("ERR_NGROK_404"));
        assert_eq!(e.to_string(), "HTTP 404 [ERR_NGROK_404]: Resource not found");
        assert_eq!(e.doc_url().unwrap(), "https://ngrok.com/docs/errors/err_ngrok_404");
        let e = parse_error(StatusCode::BAD_GATEWAY, "");
        assert_eq!(e.message, "Bad Gateway");
    }

    #[test]
    fn refuses_foreign_hosts() {
        let c = ApiClient::new("k");
        assert!(c.url("https://evil.example/x").is_err());
        assert_eq!(c.url("/endpoints").unwrap().as_str(), "https://api.ngrok.com/endpoints");
        assert_eq!(c.url("https://api.ngrok.com/endpoints?before_id=x").unwrap().query(), Some("before_id=x"));
        let secret = ApiClient::new("super_secret_key");
        assert!(!format!("{secret:?}").contains("super_secret_key"));
    }
}
