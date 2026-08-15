//! Capability-scoped HTTP client surface (`kiri.http`).
//!
//! This closes the Tauri `http` plugin parity gap (G-3) and exceeds it on the
//! security axis: every request is authorized by the central capability
//! authority (bit `HTTP`) AND constrained to a host allowlist configured by the
//! native host. Tauri's `http` plugin issues arbitrary fetches when the
//! capability is granted; Kiri additionally refuses any host that is not on the
//! explicit allowlist, so a compromised or careless frontend cannot exfiltrate
//! to an unapproved origin. Response bytes cross the control plane
//! base64-encoded and are bounded by the same bulk-object ceiling as
//! `kiri.fs`, so backpressure holds even for large responses.
//!
//! The actual transport is behind the `HttpClient` trait (mirrors
//! `ClipboardController`): the native host injects a real client; tests use a
//! stub or a loopback `StdHttpClient` and assert routing/authorization/
//! allowlist/size-cap without launching a WebView.

use std::sync::Arc;

use base64::Engine;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.http.*` commands.
pub const HTTP_CAPABILITY: u32 = 10;

/// A parsed outbound HTTP request handed to the transport.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    /// Maximum response body bytes the service will accept.
    pub max_bytes: u64,
}

/// A parsed HTTP response returned by the transport.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Transport seam. The native host provides a real network client; tests
/// provide a stub. Kept trait-based so the logical protocol has zero platform
/// or engine dependencies.
pub trait HttpClient: Send + Sync {
    fn fetch(&self, req: HttpRequest) -> Result<HttpResponse>;
}

/// Host-configured set of permitted request hosts. Default-deny: a host is
/// reachable only if it is an exact entry. This is the concrete mechanism that
/// turns Tauri's "arbitrary fetch when granted" into Kiri's "explicit allowlist
/// only".
#[derive(Debug, Clone, Default)]
pub struct HostAllowlist {
    hosts: Vec<String>,
}

impl HostAllowlist {
    pub fn new(hosts: Vec<String>) -> Self {
        Self { hosts }
    }

    /// Whether `host` (authority, optionally with port) is permitted. A
    /// trailing `:port` is ignored so `127.0.0.1:51432` matches an entry
    /// for `127.0.0.1`, which is how host-based allowlisting should behave.
    pub fn allows(&self, host: &str) -> bool {
        let host = match host.split_once(':') {
            Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
            _ => host,
        };
        self.hosts.iter().any(|h| h == host)
    }

    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }
}

/// Extract the authority (host[:port]) from an `http`/`https` URL without a URL
/// crate. Returns `None` for malformed input so the service can reject it.
fn authority_of(url: &str) -> Option<String> {
    let without_scheme = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"))?;
    let authority = without_scheme.split('/').next().unwrap_or("");
    let authority = authority.split('?').next().unwrap_or("");
    let authority = authority.split('#').next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    Some(authority.to_string())
}

/// Capability-scoped HTTP service bounded to a host allowlist plus limits.
#[derive(Clone)]
pub struct HttpService {
    client: Arc<dyn HttpClient>,
    allowlist: Arc<HostAllowlist>,
    limits: Arc<Limits>,
}

impl HttpService {
    pub fn new(client: Arc<dyn HttpClient>, allowlist: HostAllowlist, limits: Limits) -> Self {
        Self { client, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Issue a GET and return the response bounded by `max_bytes` (or the bulk
    /// ceiling when omitted). Rejects hosts outside the allowlist and responses
    /// that exceed the configured body cap.
    pub fn get(&self, req_url: &str, max_bytes: Option<u64>) -> Result<Value> {
        let authority = authority_of(req_url).ok_or_else(|| {
            Error::invalid_argument(format!("kiri.http.get: malformed url: {req_url}"))
        })?;
        if !self.allowlist.allows(&authority) {
            return Err(Error::scope_denied(format!(
                "kiri.http.get: host not on allowlist: {authority}"
            )));
        }
        let cap = max_bytes.unwrap_or(self.limits.max_single_bulk_bytes);
        let resp = self.client.fetch(HttpRequest {
            method: "GET".to_string(),
            url: req_url.to_string(),
            max_bytes: cap,
        })?;
        // Enforce the bulk-object ceiling even though the client was told the
        // cap, so a hostile or buggy transport cannot bypass backpressure.
        self.limits.check_bulk_object(resp.body.len() as u64)?;
        let headers: serde_json::Map<String, Value> =
            resp.headers.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        Ok(serde_json::json!({
            "url": req_url,
            "status": resp.status,
            "headers": headers,
            "base64": base64::engine::general_purpose::STANDARD.encode(&resp.body),
            "bytes": resp.body.len(),
        }))
    }
}

/// Build the `kiri.http.*` handlers bound to one HttpService. Reused by the
/// router builder and any plugin path so authority is identical either way.
pub fn http_handlers(
    service: HttpService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(HTTP_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::HTTP_GET,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let url = p
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::invalid_argument("kiri.http.get requires string url"))?;
            let max_bytes = p
                .get("maxBytes")
                .and_then(|v| v.as_u64())
                .or_else(|| p.get("max_bytes").and_then(|v| v.as_u64()));
            svc.get(url, max_bytes)
        }) as Handler,
    )]
}

/// Minimal blocking HTTP/1.1 GET client over std TCP. Intentionally TLS-free:
/// it backs loopback/plain tests and demonstrates the transport seam; a
/// production host substitutes a TLS-capable client behind the same trait. The
/// allowlist and capability gates above are what make the surface safe, not the
/// transport choice.
pub struct StdHttpClient;

impl HttpClient for StdHttpClient {
    fn fetch(&self, req: HttpRequest) -> Result<HttpResponse> {
        let authority = authority_of(&req.url)
            .ok_or_else(|| Error::invalid_argument(format!("http: bad url {}", req.url)))?;
        let (host, port) = if let Some((h, p)) = authority.split_once(':') {
            (h.to_string(), p.parse::<u16>().unwrap_or(80))
        } else {
            (authority.clone(), 80)
        };
        if req.url.starts_with("https://") {
            return Err(Error::command_error(
                "http: StdHttpClient supports http only (no TLS); provide a TLS client",
            ));
        }
        let path = req
            .url
            .split_once("://")
            .map(|(_, rest)| rest.split_once('/').map(|(_, p)| p).unwrap_or(""))
            .unwrap_or("");
        let path = if path.is_empty() { "/" } else { path };

        use std::io::{Read, Write};
        use std::net::TcpStream;
        let mut stream = TcpStream::connect((host.as_str(), port))
            .map_err(|e| Error::resource_not_found(format!("http connect {authority}: {e}")))?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept: */*\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| Error::command_error(format!("http write: {e}")))?;

        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .map_err(|e| Error::command_error(format!("http read: {e}")))?;
        parse_http_response(&buf)
    }
}

/// Parse a minimal HTTP/1.x response: status line, headers, and a
/// Content-Length-delimited body. Chunked encoding is rejected so we never
/// silently mis-handle a body the size cap cannot see.
fn parse_http_response(buf: &[u8]) -> Result<HttpResponse> {
    let text = String::from_utf8_lossy(buf);
    let mut lines = text.splitn(2, "\r\n\r\n");
    let head = lines.next().unwrap_or("");
    let body_start =
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(buf.len());

    let mut head_lines = head.split("\r\n");
    let status_line = head_lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| Error::command_error("http: bad status line"))?;

    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in head_lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse::<usize>().ok();
            }
            if k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked") {
                chunked = true;
            }
            headers.push((k, v));
        }
    }
    if chunked {
        return Err(Error::command_error("http: chunked encoding unsupported"));
    }
    let body = match content_length {
        Some(n) => buf.get(body_start..body_start + n).unwrap_or(&[]).to_vec(),
        None => buf[body_start..].to_vec(),
    };
    Ok(HttpResponse { status, headers, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{command_id, Router};
    use crate::trace::NoopTraceSink;
    use crate::wire::WireRequest;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    struct StubHttpClient {
        status: u16,
        body: Vec<u8>,
    }
    impl HttpClient for StubHttpClient {
        fn fetch(&self, _req: HttpRequest) -> Result<HttpResponse> {
            Ok(HttpResponse {
                status: self.status,
                headers: vec![("content-type".to_string(), "text/plain".to_string())],
                body: self.body.clone(),
            })
        }
    }

    fn router() -> Router {
        let svc = HttpService::new(
            Arc::new(StubHttpClient { status: 200, body: b"hello kiri".to_vec() }),
            HostAllowlist::new(vec!["api.example.com".to_string()]),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_http(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(HTTP_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn get_roundtrips_base64_with_status() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::HTTP_GET,
            serde_json::json!({ "url": "http://api.example.com/v1/x" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["status"], 200);
        assert_eq!(out["payload"]["bytes"], 10);
        assert_eq!(
            out["payload"]["base64"],
            base64::engine::general_purpose::STANDARD.encode(b"hello kiri")
        );
    }

    #[test]
    fn host_not_on_allowlist_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::HTTP_GET,
            serde_json::json!({ "url": "http://evil.example.net/x" }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn missing_http_capability_is_denied() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(
            command_id::HTTP_GET,
            1,
            1,
            serde_json::json!({ "url": "http://api.example.com/x" }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn response_over_size_cap_is_rejected() {
        let mut limits = Limits::default();
        limits.max_single_bulk_bytes = 100;
        let svc = HttpService::new(
            Arc::new(StubHttpClient { status: 200, body: vec![0u8; 2048] }),
            HostAllowlist::new(vec!["big.example.com".to_string()]),
            limits,
        );
        let r = Router::new_with_limits(Limits::default()).with_http(svc);
        let mut granted = CapabilityBits::empty();
        granted.set(HTTP_CAPABILITY);
        let req = WireRequest::new(
            command_id::HTTP_GET,
            1,
            1,
            serde_json::json!({ "url": "http://big.example.com/x", "maxBytes": 100 }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
    }

    #[test]
    fn real_loopback_get_with_std_client() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let body = b"loopback ok";
        thread::spawn(move || {
            let (mut s, _peer) = listener.accept().unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n",
                body.len()
            );
            s.write_all(response.as_bytes()).unwrap();
            s.write_all(body).unwrap();
            s.flush().unwrap();
            // Shut down the write side so the client's read_to_end sees EOF
            // without the OS tearing down the connection first, then yield so
            // the kernel delivers the bytes before the stream is dropped.
            let _ = s.shutdown(std::net::Shutdown::Write);
            std::thread::sleep(std::time::Duration::from_millis(30));
        });
        let host = addr.ip().to_string();
        let svc = HttpService::new(
            Arc::new(StdHttpClient),
            HostAllowlist::new(vec![host.clone()]),
            Limits::default(),
        );
        let r = Router::new_with_limits(Limits::default()).with_http(svc);
        let mut granted = CapabilityBits::empty();
        granted.set(HTTP_CAPABILITY);
        let url = format!("http://{host}:{}/path", addr.port());
        let req = WireRequest::new(command_id::HTTP_GET, 1, 1, serde_json::json!({ "url": url }));
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_none(), "unexpected error: {resp:?}");
        let payload = resp.payload.unwrap();
        assert_eq!(payload["status"], 200);
        assert_eq!(payload["bytes"], body.len());
    }
}
