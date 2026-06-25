//! HTTP response construction and serialization.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::http::status::StatusCode;

/// An HTTP response: a status, header fields, and a body.
///
/// `Content-Length` is always derived from the body at serialization time, so
/// callers never set it themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// An empty-bodied response with the given status.
    pub fn new(status: StatusCode) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Appends a header field. `Content-Length` set this way is ignored at
    /// serialization time in favor of the real body length.
    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// A `text/plain; charset=utf-8` response.
    pub fn text(status: StatusCode, body: &str) -> Response {
        Response {
            status,
            headers: header("Content-Type", "text/plain; charset=utf-8"),
            body: body.as_bytes().to_vec(),
        }
    }

    /// An `application/json` response. `body` must already be valid JSON.
    pub fn json(status: StatusCode, body: &str) -> Response {
        Response {
            status,
            headers: header("Content-Type", "application/json"),
            body: body.as_bytes().to_vec(),
        }
    }

    /// Serializes the response to wire bytes. When `head_only` is true the body
    /// is omitted (HEAD requests), but `Content-Length` still reports the full
    /// entity length so caches and clients see the same metadata as for GET.
    pub fn serialize(&self, head_only: bool) -> Vec<u8> {
        let mut head = String::new();
        head.push_str("HTTP/1.1 ");
        head.push_str(&format!(
            "{} {}\r\n",
            self.status.code(),
            self.status.reason()
        ));

        for (name, value) in &self.headers {
            // The body length is authoritative; never emit a caller's value.
            if name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            // Drop header-injection attempts: a CR, LF, or NUL in a header would
            // let an attacker forge extra headers or split the response.
            if has_forbidden_byte(name) || has_forbidden_byte(value) {
                continue;
            }
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }

        // 1xx/204/304 responses carry neither a body nor a Content-Length
        // (RFC 9112 6.3, RFC 9110 15.4.5); every other response derives its
        // length from the body.
        let empty_body = self.status.is_empty_body();
        if !empty_body {
            head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        head.push_str("\r\n");

        let mut out = head.into_bytes();
        if !head_only && !empty_body {
            out.extend_from_slice(&self.body);
        }
        out
    }
}

fn header(name: &str, value: &str) -> Vec<(String, String)> {
    Vec::from([(name.to_string(), value.to_string())])
}

/// True if a header name/value contains a byte that could split the response.
fn has_forbidden_byte(s: &str) -> bool {
    s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn serializes_status_line_and_content_length() {
        let resp = Response::text(StatusCode::OK, "hi");
        let wire = as_text(&resp.serialize(false));
        assert!(wire.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(wire.contains("Content-Type: text/plain; charset=utf-8\r\n"));
        assert!(wire.contains("Content-Length: 2\r\n"));
        assert!(wire.ends_with("\r\n\r\nhi"));
    }

    #[test]
    fn head_omits_body_but_keeps_length() {
        // HEAD must report the same Content-Length as GET would, with no body.
        let resp = Response::text(StatusCode::OK, "hello");
        let wire = as_text(&resp.serialize(true));
        assert!(wire.contains("Content-Length: 5\r\n"));
        assert!(wire.ends_with("\r\n\r\n"));
        assert!(!wire.contains("hello"));
    }

    #[test]
    fn caller_content_length_is_overridden() {
        // A wrong caller-supplied length must not reach the wire.
        let resp = Response::new(StatusCode::OK)
            .with_header("Content-Length", "999")
            .with_header("X-Test", "1");
        let mut resp = resp;
        resp.body = b"abc".to_vec();
        let wire = as_text(&resp.serialize(false));
        assert!(wire.contains("Content-Length: 3\r\n"));
        assert!(!wire.contains("999"));
        assert!(wire.contains("X-Test: 1\r\n"));
    }

    #[test]
    fn header_injection_is_dropped() {
        // A CRLF smuggled into a header value must not forge a new header.
        let resp = Response::new(StatusCode::OK)
            .with_header("X-Bad", "a\r\nInjected: 1")
            .with_header("X-Good", "ok");
        let wire = as_text(&resp.serialize(false));
        assert!(!wire.contains("Injected: 1"));
        assert!(!wire.contains("X-Bad"));
        assert!(wire.contains("X-Good: ok\r\n"));
    }

    #[test]
    fn empty_body_status_omits_body_and_length() {
        // A 304 (like 204 and 1xx) must carry neither a body nor a Content-Length,
        // even if a body was set, but keeps its validators (RFC 9112 6.3).
        let mut resp = Response::new(StatusCode::NOT_MODIFIED).with_header("ETag", "\"abc\"");
        resp.body = b"should not be sent".to_vec();
        let wire = as_text(&resp.serialize(false));
        assert!(wire.starts_with("HTTP/1.1 304 Not Modified\r\n"));
        assert!(wire.contains("ETag: \"abc\"\r\n"));
        assert!(!wire.contains("Content-Length"));
        assert!(wire.ends_with("\r\n\r\n"));
        assert!(!wire.contains("should not be sent"));
    }
}
