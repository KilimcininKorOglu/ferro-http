//! The request-handling seam between the core and a profile binary.

use crate::http::request::Request;
use crate::http::response::Response;

/// Turns a parsed request into a response.
///
/// The connection state machine ([`crate::conn::Connection`]) is generic over
/// this trait, so a profile composes its own handler (for example: try the API
/// router, then static files, then 404) while the core stays agnostic.
pub trait Service {
    fn handle(&self, request: &Request) -> Response;
}
