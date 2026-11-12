//! Presentation helpers: body decoding and rendering, curl/HAR export and
//! the editable raw-request format used by "replay with changes".

use std::{
    io::Read,
    time::{Duration, UNIX_EPOCH},
};

use base64::Engine;
use bytes::Bytes;
use serde_json::{json, Value};

use crate::{proxy::ReplayRequest, store::Exchange};

/// Undo `Content-Encoding` (gzip, x-gzip, deflate, br; chained right-to-left).
pub fn decode_body(headers: &[(String, String)], body: &[u8]) -> Result<Vec<u8>, String> {
    let enc = Exchange::header(headers, "content-encoding").unwrap_or("");
    let mut data = body.to_vec();
    for coding in enc.split(',').map(|s| s.trim().to_ascii_lowercase()).rev() {
        data = match coding.as_str() {
            "" | "identity" => data,
            "gzip" | "x-gzip" => {
                let mut out = Vec::new();
                flate2::read::MultiGzDecoder::new(&data[..]).read_to_end(&mut out).map_err(|e| format!("gzip: {e}"))?;
                out
            }
            "deflate" => {
                let mut out = Vec::new();
                // Servers send either zlib-wrapped or raw deflate.
                if flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out).is_err() {
                    out.clear();
                    flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut out).map_err(|e| format!("deflate: {e}"))?;
                }
                out
            }
            "br" => {
                let mut out = Vec::new();
                brotli::Decompressor::new(&data[..], 4096).read_to_end(&mut out).map_err(|e| format!("brotli: {e}"))?;
                out
            }
            other => return Err(format!("unsupported content-encoding `{other}`")),
        };
    }
    Ok(data)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyView {
    /// Pretty-printed when JSON / form, plain text otherwise, hex for binary.
    Auto,
    Raw,
    Hex,
}

/// Render a body for display.
pub fn render_body(headers: &[(String, String)], body: &[u8], view: BodyView) -> String {
    if body.is_empty() {
        return String::new();
    }
    let decoded = match decode_body(headers, body) {
        Ok(d) => d,
        Err(e) => return format!("[could not decode body: {e}]\n\n{}", hex_dump(body, 64 * 1024)),
    };
    let ct = Exchange::header(headers, "content-type").unwrap_or("").to_ascii_lowercase();
    match view {
        BodyView::Hex => hex_dump(&decoded, 256 * 1024),
        BodyView::Raw => match std::str::from_utf8(&decoded) {
            Ok(s) => s.to_string(),
            Err(_) => hex_dump(&decoded, 256 * 1024),
        },
        BodyView::Auto => {
            if ct.contains("json") || looks_like_json(&decoded) {
                if let Ok(v) = serde_json::from_slice::<Value>(&decoded) {
                    return serde_json::to_string_pretty(&v).unwrap_or_default();
                }
            }
            if ct.starts_with("application/x-www-form-urlencoded") {
                if let Ok(s) = std::str::from_utf8(&decoded) {
                    return url::form_urlencoded::parse(s.as_bytes()).map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n");
                }
            }
            match std::str::from_utf8(&decoded) {
                Ok(s) if !s.chars().any(|c| c.is_control() && !c.is_whitespace()) => s.to_string(),
                _ => hex_dump(&decoded, 256 * 1024),
            }
        }
    }
}

fn looks_like_json(b: &[u8]) -> bool {
    matches!(b.iter().find(|c| !c.is_ascii_whitespace()), Some(b'{') | Some(b'['))
}

/// Classic 16-bytes-per-line hex dump, capped at `max` bytes.
pub fn hex_dump(data: &[u8], max: usize) -> String {
    let mut out = String::new();
    for (i, chunk) in data[..data.len().min(max)].chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.extend(chunk.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }));
        out.push('\n');
    }
    if data.len() > max {
        out.push_str(&format!("… {} more bytes\n", data.len() - max));
    }
    out
}

pub fn format_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn format_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `curl` command reproducing the request against `base_url` (the public
/// tunnel URL or the upstream).
pub fn to_curl(e: &Exchange, base_url: &str) -> String {
    let mut parts = vec!["curl".to_string()];
    if e.method != "GET" {
        parts.push(format!("-X {}", e.method));
    }
    parts.push(shell_quote(&format!("{}{}", base_url.trim_end_matches('/'), e.uri)));
    for (k, v) in &e.request_headers {
        let lk = k.to_ascii_lowercase();
        if matches!(lk.as_str(), "host" | "content-length" | "connection" | "accept-encoding") {
            continue;
        }
        parts.push(format!("-H {}", shell_quote(&format!("{k}: {v}"))));
    }
    if !e.request_body.bytes.is_empty() {
        match std::str::from_utf8(&e.request_body.bytes) {
            Ok(s) => parts.push(format!("--data-raw {}", shell_quote(s))),
            Err(_) => parts.push("--data-binary @request.bin".into()),
        }
    }
    parts.join(" \\\n  ")
}

/// HTTP Archive 1.2 document for a set of exchanges.
pub fn to_har(exchanges: &[Exchange], base_url: &str) -> Value {
    let entries: Vec<Value> = exchanges
        .iter()
        .map(|e| {
            let started = e.started.duration_since(UNIX_EPOCH).unwrap_or_default();
            let started_rfc = httpdate::fmt_http_date(e.started);
            let hdrs = |h: &[(String, String)]| -> Vec<Value> { h.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect() };
            let body_text = |headers: &[(String, String)], b: &Bytes| -> (String, Option<&'static str>) {
                let decoded = decode_body(headers, b).unwrap_or_else(|_| b.to_vec());
                match String::from_utf8(decoded.clone()) {
                    Ok(s) => (s, None),
                    Err(_) => (base64::engine::general_purpose::STANDARD.encode(decoded), Some("base64")),
                }
            };
            let (req_text, _) = body_text(&e.request_headers, &e.request_body.bytes);
            let (resp_text, resp_enc) = body_text(&e.response_headers, &e.response_body.bytes);
            let mut content = json!({
                "size": e.response_body.total,
                "mimeType": e.response_header("content-type").unwrap_or(""),
                "text": resp_text,
            });
            if let Some(enc) = resp_enc {
                content["encoding"] = json!(enc);
            }
            let query: Vec<Value> = e
                .uri
                .split_once('?')
                .map(|(_, q)| url::form_urlencoded::parse(q.as_bytes()).map(|(k, v)| json!({ "name": k, "value": v })).collect())
                .unwrap_or_default();
            let mut request = json!({
                "method": e.method,
                "url": format!("{}{}", base_url.trim_end_matches('/'), e.uri),
                "httpVersion": e.version,
                "cookies": [],
                "headers": hdrs(&e.request_headers),
                "queryString": query,
                "headersSize": -1,
                "bodySize": e.request_body.total,
            });
            if !e.request_body.bytes.is_empty() {
                request["postData"] = json!({ "mimeType": e.request_header("content-type").unwrap_or(""), "text": req_text });
            }
            let time = e.duration.map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0);
            json!({
                "startedDateTime": started_rfc,
                "_startedEpochMs": started.as_millis() as u64,
                "time": time,
                "request": request,
                "response": {
                    "status": e.status.unwrap_or(0),
                    "statusText": e.status.and_then(|s| http::StatusCode::from_u16(s).ok()).and_then(|s| s.canonical_reason()).unwrap_or(""),
                    "httpVersion": e.version,
                    "cookies": [],
                    "headers": hdrs(&e.response_headers),
                    "content": content,
                    "redirectURL": e.response_header("location").unwrap_or(""),
                    "headersSize": -1,
                    "bodySize": e.response_body.total,
                },
                "cache": {},
                "timings": { "send": 0, "wait": time, "receive": 0 },
            })
        })
        .collect();
    json!({ "log": { "version": "1.2", "creator": { "name": "Shaft", "version": env!("CARGO_PKG_VERSION") }, "entries": entries } })
}

/// Editable text form of a request: request line, headers, blank line, body.
pub fn to_raw_request(r: &ReplayRequest) -> String {
    let mut s = format!("{} {} HTTP/1.1\n", r.method, r.path);
    for (k, v) in &r.headers {
        s.push_str(&format!("{k}: {v}\n"));
    }
    s.push('\n');
    s.push_str(&String::from_utf8_lossy(&r.body));
    s
}

/// Parse the text produced by [`to_raw_request`] (after user edits).
pub fn parse_raw_request(text: &str) -> Result<ReplayRequest, String> {
    let text = text.replace("\r\n", "\n");
    let (head, body) = match text.split_once("\n\n") {
        Some((h, b)) => (h.to_string(), b.to_string()),
        None => (text.trim_end().to_string(), String::new()),
    };
    let mut lines = head.lines();
    let request_line = lines.next().ok_or("empty request")?;
    let mut it = request_line.split_whitespace();
    let method = it.next().ok_or("missing method")?.to_ascii_uppercase();
    let path = it.next().ok_or("missing path")?.to_string();
    if !path.starts_with('/') {
        return Err(format!("path must start with `/`, got `{path}`"));
    }
    if !method.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!("invalid method `{method}`"));
    }
    let mut headers = Vec::new();
    for (i, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':').ok_or_else(|| format!("header line {} has no `:`", i + 2))?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    Ok(ReplayRequest { method, path, headers, body: Bytes::from(body) })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::store::{CapturedBody, ExchangeState};

    fn h(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn decodes_gzip_and_brotli() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"{\"a\":1}").unwrap();
        let gz = gz.finish().unwrap();
        let headers = h(&[("Content-Encoding", "gzip"), ("content-type", "application/json")]);
        assert_eq!(decode_body(&headers, &gz).unwrap(), b"{\"a\":1}");
        assert_eq!(render_body(&headers, &gz, BodyView::Auto), "{\n  \"a\": 1\n}");

        let mut br = Vec::new();
        brotli::CompressorWriter::new(&mut br, 4096, 5, 22).write_all(b"hello").unwrap();
        assert_eq!(decode_body(&h(&[("content-encoding", "br")]), &br).unwrap(), b"hello");
        assert!(decode_body(&h(&[("content-encoding", "zstd")]), b"x").is_err());
    }

    #[test]
    fn renders_form_and_binary() {
        let form = h(&[("content-type", "application/x-www-form-urlencoded")]);
        assert_eq!(render_body(&form, b"a=1&b=hello+world", BodyView::Auto), "a = 1\nb = hello world");
        let bin = render_body(&[], &[0u8, 1, 2, 0xff], BodyView::Auto);
        assert!(bin.starts_with("00000000  00 01 02 ff"));
    }

    fn exchange() -> Exchange {
        Exchange {
            id: 1,
            started: std::time::SystemTime::now(),
            duration: Some(Duration::from_millis(12)),
            remote_addr: None,
            method: "POST".into(),
            uri: "/hook?x=1".into(),
            version: "HTTP/1.1".into(),
            request_headers: h(&[("Host", "a.ngrok.app"), ("Content-Type", "application/json"), ("X-Sig", "it's")]),
            request_body: CapturedBody { bytes: Bytes::from_static(b"{\"ok\":true}"), total: 11, truncated: false, complete: true },
            status: Some(200),
            response_headers: h(&[("content-type", "text/plain")]),
            response_body: CapturedBody { bytes: Bytes::from_static(b"done"), total: 4, truncated: false, complete: true },
            state: ExchangeState::Complete,
            error: None,
            replay_of: None,
        }
    }

    #[test]
    fn curl_export() {
        let c = to_curl(&exchange(), "https://a.ngrok.app/");
        assert!(c.starts_with("curl \\\n  -X POST \\\n  'https://a.ngrok.app/hook?x=1'"));
        assert!(c.contains(r"-H 'X-Sig: it'\''s'"));
        assert!(!c.contains("Host:"));
        assert!(c.contains("--data-raw '{\"ok\":true}'"));
    }

    #[test]
    fn har_export() {
        let har = to_har(&[exchange()], "https://a.ngrok.app");
        let e = &har["log"]["entries"][0];
        assert_eq!(e["request"]["url"], "https://a.ngrok.app/hook?x=1");
        assert_eq!(e["request"]["queryString"][0]["name"], "x");
        assert_eq!(e["response"]["content"]["text"], "done");
        assert_eq!(e["response"]["statusText"], "OK");
    }

    #[test]
    fn raw_request_roundtrip() {
        let r = ReplayRequest::from_exchange(&exchange()).unwrap();
        let text = to_raw_request(&r);
        let back = parse_raw_request(&text).unwrap();
        assert_eq!(back, r);
        let edited = parse_raw_request("put /x HTTP/1.1\r\nA: b\r\n\r\nbody").unwrap();
        assert_eq!(edited.method, "PUT");
        assert_eq!(edited.body, Bytes::from_static(b"body"));
        assert!(parse_raw_request("GET x").is_err());
        assert!(parse_raw_request("GET /x\nbad header").is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.50 s");
    }
}
