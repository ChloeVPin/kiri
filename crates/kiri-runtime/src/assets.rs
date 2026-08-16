//! Window-free frontend asset server for the `kiri://` protocol.
//!
//! Pure logic: map a request path to a file under the frontend directory,
//! detect content-type from the extension, and answer `Range` requests with
//! partial content. No WebView is required to exercise any of this, so the
//! whole module is unit-testable headlessly (R-1 in docs/DEEP_AUDIT_TAURI.md).
//!
//! This replaces the previous per-request `std::fs::read` of `index.html`
//! with a hardcoded `text/html` content-type (F-1 in the audit): the old path
//! could not serve sub-assets, sent the wrong mime for everything but HTML, and
//! had no range support.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Outcome of resolving a `kiri://localhost/<path>` request.
#[derive(Debug)]
pub enum AssetResponse {
    /// 200 OK with full body and content-type.
    Full { body: Vec<u8>, content_type: String },
    /// 206 Partial Content for a single `Range: bytes=start-end`.
    Partial {
        body: Vec<u8>,
        content_type: String,
        start: u64,
        end: u64, // inclusive
        total: u64,
    },
    /// 404 Not Found.
    NotFound,
    /// 416 Range Not Satisfiable (requested range past end of file).
    RangeNotSatisfiable { total: u64 },
    /// 304 Not Modified (client ETag matched via If-None-Match).
    NotModified,
}

/// Map a file extension to a content-type. Conservative, common subset used by
/// desktop frontends. Unknown extensions fall back to `application/octet-stream`.
pub fn content_type_for(path: &Path) -> String {
    let ct = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        Some("map") => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    };
    ct.to_string()
}

/// Resolve `request_path` (the part after `kiri://localhost/`) against
/// `root`. Prevents path traversal: any `..` component is rejected.
///
/// `index.html` is served when the path is empty or `/`.
fn resolve(root: &Path, request_path: &str) -> Option<PathBuf> {
    let trimmed = request_path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some(root.join("index.html"));
    }
    let mut out = root.to_path_buf();
    for comp in Path::new(trimmed).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            // Reject ParentDir, RootDir, CurDir, and Prefix to avoid escape.
            _ => return None,
        }
    }
    Some(out)
}

/// Parse a single `bytes=start-end` range. Only one range is supported
/// (matches the common browser single-range request). Returns (start, end)
/// where end is inclusive; `end == None` means "to end of file".
fn parse_range(header: &str, total: u64) -> Option<(u64, Option<u64>)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    // Take the first range only.
    let first = spec.split(',').next()?.trim();
    if let Some((start, end)) = first.split_once('-') {
        let is_suffix = start.is_empty();
        let start = if is_suffix {
            // suffix range: bytes=-N means last N bytes; N is the part after '-'.
            let n: u64 = end.parse().ok()?;
            if n == 0 {
                return None;
            }
            total.saturating_sub(n)
        } else {
            start.parse().ok()?
        };
        // For the suffix form the text after '-' was the suffix length, not an
        // end position, so the end is open. Otherwise it is open only when the
        // part after '-' is empty (bytes=START-).
        let end = if is_suffix || end.is_empty() { None } else { Some(end.parse().ok()?) };
        Some((start, end))
    } else {
        None
    }
}

/// Serve `request_path` from `root`. `range_header` is the optional
/// Compute a stable, content-addressed ETag for a served asset. Uses FNV-1a
/// over the bytes (cheap, deterministic, collision-resistant enough for
/// HTTP caching decisions; not a cryptographic hash, which is unnecessary
/// here). Wrapped in double quotes per RFC 7232.
pub fn etag_for(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let q = "\"";
    format!("{q}{:016x}{q}", hash)
}

/// Options controlling a kiri:// asset response beyond the raw file read.
///
/// - range: optional Range header (existing behavior).
/// - if_none_match: optional If-None-Match header; when it equals the
///   computed ETag, the server returns NotModified (304) instead of the
///   body (closes the caching gap in G-4 vs Tauri cached asset protocol).
/// - allow: when non-empty, only paths whose resolved sub-path starts with
///   one of these prefixes are served; everything else is NotFound. This is
///   the kiri:// origin allowlist parity with Tauri asset:customProtocol
///   allowlist (G-4).
#[derive(Default)]
pub struct ServeOptions<'a> {
    pub range: Option<&'a str>,
    pub if_none_match: Option<&'a str>,
    pub allow: &'a [&'a str],
}

/// Serve with full policy: allowlist, range, and conditional (ETag) caching.
pub fn serve_checked(root: &Path, request_path: &str, opts: &ServeOptions) -> AssetResponse {
    let path = match resolve(root, request_path) {
        Some(p) => p,
        None => return AssetResponse::NotFound,
    };
    if !opts.allow.is_empty() {
        let sub = request_path.trim_start_matches('/');
        let allowed = opts.allow.iter().any(|prefix| {
            sub == *prefix || sub.starts_with(&format!("{prefix}/", prefix = prefix))
        });
        if !allowed {
            return AssetResponse::NotFound;
        }
    }
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return AssetResponse::NotFound,
    };
    serve_from_bytes(&path, &bytes, opts)
}

/// Serve already-loaded bytes (disk or compile-time embed) with the same
/// Range / ETag / allowlist policy as [`serve_checked`].
pub fn serve_from_bytes(path: &Path, bytes: &[u8], opts: &ServeOptions) -> AssetResponse {
    let total = bytes.len() as u64;
    let content_type = content_type_for(path);
    if let Some(client_etag) = opts.if_none_match {
        if client_etag == etag_for(bytes) {
            return AssetResponse::NotModified;
        }
    }
    if let Some(rh) = opts.range {
        let (start, end_opt) = match parse_range(rh, total) {
            Some(v) => v,
            None => return AssetResponse::RangeNotSatisfiable { total },
        };
        let end = match end_opt {
            Some(e) => {
                if e >= total {
                    total.saturating_sub(1)
                } else {
                    e
                }
            }
            None => total.saturating_sub(1),
        };
        if start > end || total == 0 {
            return AssetResponse::RangeNotSatisfiable { total };
        }
        let end_usize = end as usize;
        let body = bytes[start as usize..=end_usize].to_vec();
        return AssetResponse::Partial { body, content_type, start, end, total };
    }
    AssetResponse::Full { body: bytes.to_vec(), content_type }
}

/// Serve a request from the compile-time packed frontend.
pub fn serve_embedded(request_path: &str, opts: &ServeOptions) -> AssetResponse {
    if !opts.allow.is_empty() {
        let sub = request_path.trim_start_matches('/');
        let allowed = opts.allow.iter().any(|prefix| {
            sub == *prefix || sub.starts_with(&format!("{prefix}/", prefix = prefix))
        });
        if !allowed && !sub.is_empty() {
            return AssetResponse::NotFound;
        }
    }
    let key = match crate::embed::normalize_key(request_path) {
        Some(k) => k,
        None => return AssetResponse::NotFound,
    };
    let bytes = match crate::embed::get(&key) {
        Some(b) => b,
        None => return AssetResponse::NotFound,
    };
    serve_from_bytes(Path::new(&key), bytes, opts)
}

/// `Range` request header. This is the single entry point used by both the
/// WebView protocol closure and the unit tests.
pub fn serve(root: &Path, request_path: &str, range_header: Option<&str>) -> AssetResponse {
    let path = match resolve(root, request_path) {
        Some(p) => p,
        None => return AssetResponse::NotFound,
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return AssetResponse::NotFound,
    };
    let total = bytes.len() as u64;
    let content_type = content_type_for(&path);

    if let Some(rh) = range_header {
        let (start, end_opt) = match parse_range(rh, total) {
            Some(v) => v,
            None => return AssetResponse::RangeNotSatisfiable { total },
        };
        let end = match end_opt {
            Some(e) => {
                if e >= total {
                    total.saturating_sub(1)
                } else {
                    e
                }
            }
            None => total.saturating_sub(1),
        };
        if start > end || total == 0 {
            return AssetResponse::RangeNotSatisfiable { total };
        }
        let end_usize = end as usize;
        let body = bytes[start as usize..=end_usize].to_vec();
        return AssetResponse::Partial { body, content_type, start, end, total };
    }

    AssetResponse::Full { body: bytes, content_type }
}

/// Build the HTTP response headers (as a `Vec<(String, String)>`) for an
/// `AssetResponse`. Kept separate from the body so the WebView closure and the
/// tests can assert on headers independently.
pub fn response_headers(resp: &AssetResponse) -> Vec<(String, String)> {
    let mut h = Vec::new();
    match resp {
        AssetResponse::Full { body, content_type } => {
            h.push(("Content-Type".into(), content_type.clone()));
            h.push(("ETag".into(), etag_for(body)));
            h.push(("Cache-Control".into(), "public, max-age=0, must-revalidate".into()));
        }
        AssetResponse::Partial { body, content_type, start, end, total } => {
            h.push(("Content-Type".into(), content_type.clone()));
            h.push(("Content-Range".into(), format!("bytes {start}-{end}/{total}")));
            h.push(("Accept-Ranges".into(), "bytes".into()));
            h.push(("ETag".into(), etag_for(body)));
        }
        AssetResponse::NotFound => {
            h.push(("Content-Type".into(), "text/plain".into()));
        }
        AssetResponse::RangeNotSatisfiable { total } => {
            h.push(("Content-Range".into(), format!("bytes */{total}")));
        }
        AssetResponse::NotModified => {
            h.push(("Cache-Control".into(), "public, max-age=0, must-revalidate".into()));
        }
    }
    h
}

/// Status code for an `AssetResponse`.
pub fn status_code(resp: &AssetResponse) -> u16 {
    match resp {
        AssetResponse::Full { .. } => 200,
        AssetResponse::Partial { .. } => 206,
        AssetResponse::NotFound => 404,
        AssetResponse::RangeNotSatisfiable { .. } => 416,
        AssetResponse::NotModified => 304,
    }
}

/// Convenience: build a `kiri://localhost` request path -> response map for
/// callers that want to pre-seed a virtual filesystem (tests). Not used by the
/// runtime directly; kept for symmetry/testing.
pub type VirtualFs = HashMap<String, Vec<u8>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_root() -> (PathBuf, PathBuf) {
        // Manual temp dir (no external crate): unique subdir under the system
        // temp dir, removed at the end of the test via Drop below.
        // Monotonic sequence (not wall-clock) so parallel tests never share a
        // temp dir; two tests created within the same SystemTime granule on
        // macOS would otherwise collide and one's cleanup would delete the
        // other's files, causing non-deterministic panics.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("kiri-assets-test-{}-{}", std::process::id(), seq));
        std::fs::create_dir_all(&base).unwrap();
        let root = base.clone();
        let mut f = std::fs::File::create(root.join("index.html")).unwrap();
        f.write_all(b"<html>kiri</html>").unwrap();
        let mut f = std::fs::File::create(root.join("app.js")).unwrap();
        f.write_all(b"console.log(1)").unwrap();
        let mut f = std::fs::File::create(root.join("style.css")).unwrap();
        f.write_all(b"body{}").unwrap();
        let mut f = std::fs::File::create(root.join("img.svg")).unwrap();
        f.write_all(b"<svg></svg>").unwrap();
        (base, root)
    }

    struct _Cleanup(PathBuf);
    impl Drop for _Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn serves_index_for_empty_path() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve(&root, "", None);
        match r {
            AssetResponse::Full { body, content_type } => {
                assert_eq!(body, b"<html>kiri</html>");
                assert!(content_type.starts_with("text/html"));
            }
            _ => panic!("expected Full"),
        }
    }

    #[test]
    fn serves_subasset_with_correct_mime() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve(&root, "/app.js", None);
        match r {
            AssetResponse::Full { body, content_type } => {
                assert_eq!(body, b"console.log(1)");
                assert!(content_type.contains("javascript"), "got {content_type}");
            }
            _ => panic!("expected Full"),
        }
        let r = serve(&root, "style.css", None);
        assert!(matches!(r, AssetResponse::Full { .. }));
        let r = serve(&root, "img.svg", None);
        match r {
            AssetResponse::Full { content_type, .. } => assert!(content_type.contains("svg")),
            _ => panic!(),
        }
    }

    #[test]
    fn unknown_extension_is_octet_stream() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let mut f = std::fs::File::create(root.join("blob.dat")).unwrap();
        f.write_all(b"x").unwrap();
        let r = serve(&root, "blob.dat", None);
        match r {
            AssetResponse::Full { content_type, .. } => {
                assert_eq!(content_type, "application/octet-stream")
            }
            _ => panic!(),
        }
    }

    #[test]
    fn not_found_for_missing_file() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        assert!(matches!(serve(&root, "nope.js", None), AssetResponse::NotFound));
    }

    #[test]
    fn path_traversal_rejected() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        // `..` must not escape the root.
        assert!(matches!(serve(&root, "../../etc/passwd", None), AssetResponse::NotFound));
        assert!(matches!(serve(&root, "/../Cargo.toml", None), AssetResponse::NotFound));
    }

    #[test]
    fn range_full_span() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve(&root, "app.js", Some("bytes=0-13"));
        match &r {
            AssetResponse::Partial { body, start, end, total, .. } => {
                assert_eq!(body, b"console.log(1)");
                assert_eq!((*start, *end, *total), (0, 13, 14));
            }
            _ => panic!("expected Partial, got {r:?}"),
        }
        assert_eq!(status_code(&r), 206);
    }

    #[test]
    fn range_suffix() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve(&root, "app.js", Some("bytes=-3"));
        match r {
            AssetResponse::Partial { body, start, end, total, .. } => {
                assert_eq!(body, b"(1)");
                assert_eq!((start, end, total), (11, 13, 14));
            }
            _ => panic!("expected Partial"),
        }
    }

    #[test]
    fn range_open_end() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve(&root, "app.js", Some("bytes=4-"));
        match r {
            AssetResponse::Partial { body, start, end, total, .. } => {
                assert_eq!(body, b"ole.log(1)");
                assert_eq!((start, end, total), (4, 13, 14));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn range_past_end_is_416() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve(&root, "app.js", Some("bytes=20-30"));
        assert!(matches!(r, AssetResponse::RangeNotSatisfiable { total: 14 }));
        assert_eq!(status_code(&r), 416);
    }

    #[test]
    fn range_header_present_in_partial() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve(&root, "app.js", Some("bytes=0-3"));
        let headers = response_headers(&r);
        assert!(headers.iter().any(|(k, v)| k == "Content-Range" && v == "bytes 0-3/14"));
        assert!(headers.iter().any(|(k, _)| k == "Accept-Ranges"));
    }

    // R-3: the shipped frontend JS surface (kiri.js) must be served over the
    // kiri:// protocol with the correct JavaScript mime type and expose the
    // platform/app/event API. This exercises the real examples/blank asset so
    // the shipped surface is verified, not a synthetic copy.
    #[test]
    fn embedded_index_and_js_match_blank_frontend() {
        let index = serve_embedded("/", &ServeOptions::default());
        match index {
            AssetResponse::Full { content_type, body } => {
                assert!(content_type.starts_with("text/html"));
                assert!(!body.is_empty());
            }
            other => panic!("expected Full index, got {:?}", other),
        }
        let js = serve_embedded("kiri.js", &ServeOptions::default());
        match js {
            AssetResponse::Full { content_type, .. } => {
                assert_eq!(content_type, "text/javascript; charset=utf-8");
            }
            other => panic!("expected Full kiri.js, got {:?}", other),
        }
        assert!(matches!(
            serve_embedded("../secret", &ServeOptions::default()),
            AssetResponse::NotFound
        ));
    }

    #[test]
    fn blank_frontend_serves_kiri_js_with_javascript_mime() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/blank");
        assert!(root.join("kiri.js").exists(), "examples/blank/kiri.js must exist");
        let r = serve(&root, "kiri.js", None);
        match &r {
            AssetResponse::Full { body, content_type } => {
                assert_eq!(content_type, "text/javascript; charset=utf-8");
                let text = String::from_utf8_lossy(body);
                assert!(text.contains("global.__kiri"), "kiri.js must expose __kiri");
                assert!(text.contains("platform") && text.contains("event"));
            }
            other => panic!("expected Full, got {:?}", other),
        }
    }
    // R-4 (G-4 parity): conditional GET returns 304 when the client ETag
    // matches, and the full response carries an ETag + Cache-Control.
    #[test]
    fn conditional_get_returns_304_on_etag_match() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        let r = serve_checked(
            &root,
            "app.js",
            &ServeOptions { range: None, if_none_match: None, allow: &[] },
        );
        let etag = match &r {
            AssetResponse::Full { body, .. } => etag_for(body),
            other => panic!("expected Full, got {:?}", other),
        };
        assert!(!etag.is_empty());
        let headers = response_headers(&r);
        assert!(headers.iter().any(|(k, _)| k == "ETag"));
        assert!(headers.iter().any(|(k, _)| k == "Cache-Control"));

        let r2 = serve_checked(
            &root,
            "app.js",
            &ServeOptions { range: None, if_none_match: Some(&etag), allow: &[] },
        );
        assert!(matches!(r2, AssetResponse::NotModified));
        assert_eq!(status_code(&r2), 304);
    }

    #[test]
    fn etag_is_stable_for_same_bytes() {
        assert_eq!(etag_for(b"hello"), etag_for(b"hello"));
        assert_ne!(etag_for(b"hello"), etag_for(b"hellp"));
    }

    // R-4 (G-4 parity): the kiri:// origin allowlist restricts served paths.
    #[test]
    fn allowlist_only_serves_permitted_prefixes() {
        let (dir, root) = tmp_root();
        let _guard = _Cleanup(dir);
        // Only allow exactly "app.js" and the "assets/" prefix.
        let allow = ["app.js", "assets/"];
        let opts = ServeOptions { range: None, if_none_match: None, allow: &allow };
        assert!(matches!(serve_checked(&root, "app.js", &opts), AssetResponse::Full { .. }));
        assert!(matches!(serve_checked(&root, "assets/icon.png", &opts), AssetResponse::NotFound));
        // Disallowed prefix is rejected even though the file exists.
        assert!(matches!(serve_checked(&root, "style.css", &opts), AssetResponse::NotFound));
        // Empty allowlist means allow everything.
        assert!(matches!(
            serve_checked(&root, "style.css", &ServeOptions::default()),
            AssetResponse::Full { .. }
        ));
    }
}
