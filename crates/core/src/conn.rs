//! Per-connection state machine driving parse -> dispatch -> response.
//!
//! The state machine is transport-agnostic and non-blocking: a profile binary
//! feeds it received bytes via [`Connection::feed`] and repeatedly calls
//! [`Connection::step`], writing back any produced bytes. This keeps all
//! protocol logic in the core, identical across the std (event loop) and
//! embedded (smoltcp) profiles.

use alloc::vec::Vec;

use crate::http::request::{parse, ParseError, Parsed};
use crate::http::response::Response;
use crate::http::status::StatusCode;
use crate::service::Service;

/// The result of advancing a connection by one step.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// More bytes are needed from the peer before progress can be made.
    NeedMore,
    /// Bytes to write back to the peer. `close` signals that the connection
    /// must be closed once these bytes are flushed.
    Write { bytes: Vec<u8>, close: bool },
}

/// A single client connection accumulating bytes until a request is complete.
#[derive(Default)]
pub struct Connection {
    buf: Vec<u8>,
}

impl Connection {
    /// Creates an empty connection state.
    pub fn new() -> Connection {
        Connection { buf: Vec::new() }
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
    pub fn step<S: Service>(&mut self, service: &S) -> Step {
        match parse(&self.buf) {
            Ok(Parsed::Partial) => Step::NeedMore,
            Ok(Parsed::Complete { request, consumed }) => {
                self.buf.drain(..consumed);
                let keep_alive = request.keep_alive();
                let head_only = request.method.is_head();
                let response = service.handle(&request);
                Step::Write {
                    bytes: response.serialize(head_only),
                    close: !keep_alive,
                }
            }
            Err(err) => {
                let status = error_status(err);
                let response = Response::text(status, status.reason());
                // An unparseable stream cannot be safely resynchronized; close.
                Step::Write {
                    bytes: response.serialize(false),
                    close: true,
                }
            }
        }
    }
}

/// Maps a parse error to the status code reported to the client.
fn error_status(err: ParseError) -> StatusCode {
    match err {
        ParseError::UnsupportedVersion => StatusCode::HTTP_VERSION_NOT_SUPPORTED,
        ParseError::UnsupportedTransferEncoding => StatusCode::NOT_IMPLEMENTED,
        ParseError::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ParseError::TargetTooLong => StatusCode::URI_TOO_LONG,
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
        assert_eq!(conn.step(&router()), Step::NeedMore);
    }

    #[test]
    fn complete_request_is_dispatched() {
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
        let step = conn.step(&router());
        match &step {
            Step::Write { close, .. } => assert!(*close, "Connection: close must close"),
            Step::NeedMore => panic!("expected write"),
        }
        assert!(wire(&step).starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn keep_alive_request_does_not_close() {
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/1.1\r\n\r\n");
        match conn.step(&router()) {
            Step::Write { close, .. } => assert!(!close, "HTTP/1.1 defaults to keep-alive"),
            Step::NeedMore => panic!("expected write"),
        }
    }

    #[test]
    fn unknown_route_yields_404() {
        let mut conn = Connection::new();
        conn.feed(b"GET /missing HTTP/1.1\r\n\r\n");
        assert!(wire(&conn.step(&router())).starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[test]
    fn parse_error_responds_and_closes() {
        let mut conn = Connection::new();
        conn.feed(b"GET / HTTP/2.0\r\n\r\n");
        let step = conn.step(&router());
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
        conn.feed(b"GET / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\n");
        assert!(matches!(conn.step(&router()), Step::Write { .. }));
        assert!(matches!(conn.step(&router()), Step::Write { .. }));
        assert_eq!(conn.step(&router()), Step::NeedMore);
    }
}
