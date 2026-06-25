//! Per-connection state machine driving parse -> dispatch -> response.
//!
//! The state machine is transport-agnostic and non-blocking: a profile binary
//! feeds it received bytes via [`Connection::feed`] and repeatedly calls
//! [`Connection::step`], writing back any produced bytes. This keeps all
//! protocol logic in the core, identical across the std (event loop) and
//! embedded (smoltcp) profiles.

use alloc::vec::Vec;

use crate::http::date::http_date;
use crate::http::request::{
    expect_action, parse_with, ExpectAction, ParseError, Parsed, MAX_BODY_BYTES,
};
use crate::http::response::Response;
use crate::http::status::StatusCode;
use crate::service::{RequestContext, Service};

/// The result of advancing a connection by one step.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// More bytes are needed from the peer before progress can be made.
    NeedMore,
    /// Bytes to write back to the peer. `close` signals that the connection
    /// must be closed once these bytes are flushed.
    Write { bytes: Vec<u8>, close: bool },
}

/// Response-finalization policy applied to every response on a connection.
#[derive(Debug, Clone, Copy)]
pub struct ResponsePolicy {
    /// Whether to attach the standard security headers.
    pub security_headers: bool,
    /// Whether the server has a real clock. A clockless origin server must not
    /// emit a `Date` header (RFC 9110 6.6.1); defaults to `true`.
    pub has_clock: bool,
}

impl Default for ResponsePolicy {
    fn default() -> ResponsePolicy {
        ResponsePolicy {
            security_headers: false,
            has_clock: true,
        }
    }
}

/// A single client connection accumulating bytes until a request is complete.
pub struct Connection {
    buf: Vec<u8>,
    policy: ResponsePolicy,
    max_body: usize,
    peer: [u8; 16],
    /// Whether an interim 100 Continue has already been sent for the in-flight
    /// request, so it is emitted at most once.
    continue_sent: bool,
}

impl Default for Connection {
    fn default() -> Connection {
        Connection::new()
    }
}

impl Connection {
    /// Creates an empty connection state with the default policy and body limit.
    pub fn new() -> Connection {
        Connection {
            buf: Vec::new(),
            policy: ResponsePolicy::default(),
            max_body: MAX_BODY_BYTES,
            peer: [0u8; 16],
            continue_sent: false,
        }
    }

    /// Creates an empty connection state with an explicit response policy.
    pub fn with_policy(policy: ResponsePolicy) -> Connection {
        Connection {
            buf: Vec::new(),
            policy,
            max_body: MAX_BODY_BYTES,
            peer: [0u8; 16],
            continue_sent: false,
        }
    }

    /// Sets the maximum request body size (bytes) accepted on this connection.
    pub fn max_body(mut self, max_body: usize) -> Connection {
        self.max_body = max_body;
        self
    }

    /// Sets the peer IP (16 bytes, IPv4 IPv6-mapped) for rate limiting and logs.
    pub fn peer(mut self, peer: [u8; 16]) -> Connection {
        self.peer = peer;
        self
    }

    /// Declares whether the server has a real clock. When false, responses omit
    /// the `Date` header (RFC 9110 6.6.1), for boards without an RTC.
    pub fn clock(mut self, has_clock: bool) -> Connection {
        self.policy.has_clock = has_clock;
        self
    }

    /// Appends newly received bytes to the connection buffer.
    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Attempts to handle one buffered request, dispatching through `service`.
    ///
    /// Returns [`Step::NeedMore`] when the buffer holds only a partial request,
    /// or [`Step::Write`] with the serialized response otherwise. A parse error
    /// yields an error response and closes the connection.
    pub fn step<S: Service>(&mut self, service: &S, now_unix_secs: u64) -> Step {
        let policy = self.policy;
        let ctx = RequestContext {
            peer: self.peer,
            now_unix_secs,
        };
        match parse_with(&self.buf, self.max_body) {
            // While the body is still arriving, honor an Expect header: send an
            // interim 100 Continue once, or refuse an unsupported expectation
            // with 417, before the client commits to sending the body (RFC 9110
            // 10.1.1).
            Ok(Parsed::Partial) => match expect_action(&self.buf) {
                ExpectAction::Continue if !self.continue_sent => {
                    self.continue_sent = true;
                    Step::Write {
                        bytes: Response::new(StatusCode::CONTINUE).serialize(false),
                        close: false,
                    }
                }
                ExpectAction::Unsupported => {
                    let response = finalize(
                        Response::text(StatusCode::EXPECTATION_FAILED, "417 Expectation Failed"),
                        now_unix_secs,
                        false,
                        &policy,
                    );
                    Step::Write {
                        bytes: response.serialize(false),
                        close: true,
                    }
                }
                _ => Step::NeedMore,
            },
            Ok(Parsed::Complete { request, consumed }) => {
                self.buf.drain(..consumed);
                self.continue_sent = false;
                let keep_alive = request.keep_alive();
                let head_only = request.method.is_head();
                let response = finalize(
                    service.handle(&request, &ctx),
                    now_unix_secs,
                    keep_alive,
                    &policy,
                );
                Step::Write {
                    bytes: response.serialize(head_only),
                    close: !keep_alive,
                }
            }
            Err(err) => {
                let status = error_status(err);
                let response = finalize(
                    Response::text(status, status.reason()),
                    now_unix_secs,
                    false,
                    &policy,
                );
                // An unparseable stream cannot be safely resynchronized; close.
                Step::Write {
                    bytes: response.serialize(false),
                    close: true,
                }
            }
        }
    }
}

/// Adds the protocol-level `Date` and `Connection` headers, plus the standard
/// security headers when the policy enables them.
fn finalize(
    response: Response,
    now_unix_secs: u64,
    keep_alive: bool,
    policy: &ResponsePolicy,
) -> Response {
    let connection = if keep_alive { "keep-alive" } else { "close" };
    let mut response = response.with_header("Connection", connection);
    // A clockless origin server must not emit Date (RFC 9110 6.6.1).
    if policy.has_clock {
        response = response.with_header("Date", &http_date(now_unix_secs));
    }
    if policy.security_headers {
        apply_security_headers(response)
    } else {
        response
    }
}

/// Attaches a modern, conservative set of security headers.
fn apply_security_headers(response: Response) -> Response {
    response
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("X-Frame-Options", "SAMEORIGIN")
        .with_header("Referrer-Policy", "strict-origin-when-cross-origin")
}

/// Maps a parse error to the status code reported to the client.
fn error_status(err: ParseError) -> StatusCode {
    match err {
        ParseError::UnsupportedVersion => StatusCode::HTTP_VERSION_NOT_SUPPORTED,
        // An unrecognized but well-formed method token is "not implemented"
        // (RFC 9110 15.6.2), distinct from a malformed request (400).
        ParseError::UnknownMethod | ParseError::UnsupportedTransferEncoding => {
            StatusCode::NOT_IMPLEMENTED
        }
        ParseError::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ParseError::TargetTooLong => StatusCode::URI_TOO_LONG,
        // An oversized request head or too many fields (RFC 6585).
        ParseError::HeadTooLarge | ParseError::TooManyHeaders => {
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE
        }
        _ => StatusCode::BAD_REQUEST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::method::Method;
    use crate::http::request::Request;
    use crate::router::{Params, Router};
    use alloc::string::String;

    fn ok_handler(_req: &Request, _p: &Params) -> Response {
        Response::text(StatusCode::OK, "ok")
    }

    fn router() -> Router {
        let mut r = Router::new();
        r.route(Method::Get, "/", ok_handler);
        r
    }

    fn wire(step: &Step) -> String {
        match step {
            Step::Write { bytes, .. } => String::from_utf8(bytes.clone()).unwrap(),
            Step::NeedMore => panic!("expected write"),
        }
    }

    #[test]
    fn partial_request_needs_more() {
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/1.1\r\n");
        assert_eq!(conn.step(&router(), 0), Step::NeedMore);
    }

    #[test]
    fn complete_request_is_dispatched() {
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n");
        let step = conn.step(&router(), 0);
        match &step {
            Step::Write { close, .. } => assert!(*close, "Connection: close must close"),
            Step::NeedMore => panic!("expected write"),
        }
        assert!(wire(&step).starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn keep_alive_request_does_not_close() {
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/1.1\r\nHost: h\r\n\r\n");
        match conn.step(&router(), 0) {
            Step::Write { close, .. } => assert!(!close, "HTTP/1.1 defaults to keep-alive"),
            Step::NeedMore => panic!("expected write"),
        }
    }

    #[test]
    fn unknown_route_yields_404() {
        let mut conn = Connection::new();
        conn.feed(b"GET /missing HTTP/1.1\r\nHost: h\r\n\r\n");
        assert!(wire(&conn.step(&router(), 0)).starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[test]
    fn parse_error_responds_and_closes() {
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/2.0\r\n\r\n");
        let step = conn.step(&router(), 0);
        match &step {
            Step::Write { close, .. } => assert!(*close),
            Step::NeedMore => panic!("expected write"),
        }
        assert!(wire(&step).starts_with("HTTP/1.1 505 "));
    }

    #[test]
    fn pipelined_requests_handled_across_steps() {
        // Two keep-alive requests arrive together; each step handles one.
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/1.1\r\nHost: h\r\n\r\nGET / HTTP/1.1\r\nHost: h\r\n\r\n");
        assert!(matches!(conn.step(&router(), 0), Step::Write { .. }));
        assert!(matches!(conn.step(&router(), 0), Step::Write { .. }));
        assert_eq!(conn.step(&router(), 0), Step::NeedMore);
    }

    #[test]
    fn response_carries_date_and_connection_headers() {
        // Every response gets a Date and an explicit Connection header.
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n");
        let text = wire(&conn.step(&router(), 0));
        assert!(text.contains("Date: Thu, 01 Jan 1970 00:00:00 GMT\r\n"));
        assert!(text.contains("Connection: close\r\n"));
    }

    #[test]
    fn security_headers_applied_only_when_policy_enables() {
        // Default policy: no security headers.
        let mut plain = Connection::new();
        plain.feed(b"GET / HTTP/1.1\r\nHost: h\r\n\r\n");
        assert!(!wire(&plain.step(&router(), 0)).contains("X-Content-Type-Options"));

        // Enabled policy: standard security headers attached, even on errors.
        let policy = ResponsePolicy {
            security_headers: true,
            ..ResponsePolicy::default()
        };
        let mut secured = Connection::with_policy(policy);
        secured.feed(b"GET / HTTP/2.0\r\n\r\n"); // a 505 error path
        let text = wire(&secured.step(&router(), 0));
        assert!(text.starts_with("HTTP/1.1 505 "));
        assert!(text.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(text.contains("X-Frame-Options: SAMEORIGIN\r\n"));
        assert!(text.contains("Referrer-Policy: strict-origin-when-cross-origin\r\n"));
    }

    #[test]
    fn unknown_method_yields_501() {
        // A well-formed but unimplemented method is 501, not 400 (RFC 9110 15.6.2).
        let mut conn = Connection::new();
        conn.feed(b"PROPFIND / HTTP/1.1\r\nHost: h\r\n\r\n");
        assert!(wire(&conn.step(&router(), 0)).starts_with("HTTP/1.1 501 "));
    }

    #[test]
    fn oversized_head_yields_431() {
        // An over-limit header block is 431 (RFC 6585), not a generic 400.
        let mut conn = Connection::new();
        let mut req = Vec::from(&b"GET / HTTP/1.1\r\nHost: h\r\n"[..]);
        while req.len() <= 16 * 1024 {
            req.extend_from_slice(b"X-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        }
        req.extend_from_slice(b"\r\n");
        conn.feed(&req);
        assert!(wire(&conn.step(&router(), 0)).starts_with("HTTP/1.1 431 "));
    }

    #[test]
    fn expect_100_continue_is_interim_then_dispatches() {
        // RFC 9110 10.1.1: the head with Expect: 100-continue must draw an interim
        // 100 (connection stays open), exactly once, before the body arrives; the
        // body then produces the final response.
        let mut conn = Connection::new();
        conn.feed(
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 3\r\nExpect: 100-continue\r\n\r\n",
        );
        let interim = conn.step(&router(), 0);
        match &interim {
            Step::Write { close, .. } => {
                assert!(!close, "100 Continue must keep the connection open")
            }
            Step::NeedMore => panic!("expected an interim 100"),
        }
        assert_eq!(wire(&interim), "HTTP/1.1 100 Continue\r\n\r\n");
        // No duplicate 100 while still waiting for the body.
        assert_eq!(conn.step(&router(), 0), Step::NeedMore);
        // The body arrives -> the final response is produced.
        conn.feed(b"abc");
        assert!(matches!(conn.step(&router(), 0), Step::Write { .. }));
    }

    #[test]
    fn clockless_server_omits_date() {
        // RFC 9110 6.6.1: a server without a real clock must not emit Date, even
        // though it still sends Connection.
        let mut conn = Connection::new().clock(false);
        conn.feed(b"GET / HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n");
        let text = wire(&conn.step(&router(), 0));
        assert!(
            !text.contains("Date:"),
            "clockless server must not send Date"
        );
        assert!(text.contains("Connection: close\r\n"));
    }

    #[test]
    fn unsupported_expectation_yields_417() {
        // An expectation the server cannot meet is refused with 417, not silently
        // ignored (RFC 9110 10.1.1).
        let mut conn = Connection::new();
        conn.feed(
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 3\r\nExpect: the-impossible\r\n\r\n",
        );
        let step = conn.step(&router(), 0);
        assert!(wire(&step).starts_with("HTTP/1.1 417 "));
        match step {
            Step::Write { close, .. } => assert!(close),
            Step::NeedMore => panic!("expected a 417"),
        }
    }
}
