//! The request-handling seam between the core and a profile binary.

use crate::http::request::Request;
use crate::http::response::Response;

/// Per-request connection metadata the core threads through to the handler.
///
/// Carries what a [`Service`] needs beyond the request itself: the peer address
/// (for rate limiting and logging) and the request timestamp.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestContext {
    /// Peer IP as 16 bytes (IPv4 addresses are IPv6-mapped); zeroed if unknown.
    pub peer: [u8; 16],
    /// Wall-clock seconds since the Unix epoch for this request.
    pub now_unix_secs: u64,
}

/// Turns a parsed request into a response.
///
/// The connection state machine ([`crate::conn::Connection`]) is generic over
/// this trait, so a profile composes its own handler (for example: rate limit,
/// then the API router, then static files, then 404) while the core stays
/// agnostic.
pub trait Service {
    fn handle(&self, request: &Request, ctx: &RequestContext) -> Response;
}
