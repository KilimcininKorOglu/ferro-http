//! The std-profile request handler: rate limit, API router, static files, 404,
//! with optional gzip compression and access logging.

use std::net::Ipv6Addr;
use std::sync::Mutex;

use ferro_core::config::CompressionConfig;
use ferro_core::handler::static_files::serve_static;
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::router::Router;
use ferro_core::security::{Decision, RateLimiter};
use ferro_core::service::{RequestContext, Service};

use crate::fs_assets::FsAssets;

/// Composes per-peer rate limiting, the API router, filesystem static serving,
/// optional gzip compression, and access logging.
pub struct App {
    router: Router,
    assets: FsAssets,
    index_files: Vec<String>,
    mime_overrides: Vec<(String, String)>,
    rate_limiter: Option<Mutex<RateLimiter>>,
    access_log: bool,
    #[cfg_attr(not(feature = "gzip"), allow(dead_code))]
    compression: CompressionConfig,
}

impl App {
    /// Builds the application from its parts. The rate limiter, if present, is
    /// shared across reactor threads behind a mutex.
    pub fn new(
        router: Router,
        assets: FsAssets,
        index_files: Vec<String>,
        mime_overrides: Vec<(String, String)>,
        rate_limiter: Option<RateLimiter>,
        access_log: bool,
        compression: CompressionConfig,
    ) -> App {
        App {
            router,
            assets,
            index_files,
            mime_overrides,
            rate_limiter: rate_limiter.map(Mutex::new),
            access_log,
            compression,
        }
    }

    /// API routes first, then static files (GET/HEAD), then method-aware
    /// discovery (OPTIONS / 405 + `Allow`) for known router paths, then 404.
    fn dispatch(&self, request: &Request) -> Response {
        if let Some(response) = self.router.dispatch(request) {
            return response;
        }
        if matches!(request.method, Method::Get | Method::Head) {
            if let Some(response) = serve_static(
                request.path(),
                &self.index_files,
                &self.assets,
                &self.mime_overrides,
            ) {
                return response;
            }
        }
        // The path is a known router resource but the method did not match:
        // answer OPTIONS with its methods and reject the rest with 405 + Allow
        // (RFC 9110 Section 15.5.6; QUERY discovery per RFC 10008 Appendix A.2).
        // Static-only paths are not router resources and fall through to 404.
        let allowed = self.router.allowed_methods(request.path());
        if !allowed.is_empty() {
            let allow = allow_header_value(&allowed);
            if request.method == Method::Options {
                return Response::new(StatusCode::OK).with_header("Allow", &allow);
            }
            return Response::text(StatusCode::METHOD_NOT_ALLOWED, "405 Method Not Allowed")
                .with_header("Allow", &allow);
        }
        Response::text(StatusCode::NOT_FOUND, "404 Not Found")
    }

    #[cfg(feature = "gzip")]
    fn post_process(&self, request: &Request, response: Response) -> Response {
        if !self.compression.gzip
            || response.body.len() < self.compression.min_bytes
            || !accepts_gzip(request)
            || !is_compressible(&response)
            || has_header(&response, "content-encoding")
        {
            return response;
        }
        let mut response = response;
        response.body = ferro_core::compress::gzip_encode(&response.body);
        response
            .with_header("Content-Encoding", "gzip")
            .with_header("Vary", "Accept-Encoding")
    }

    #[cfg(not(feature = "gzip"))]
    fn post_process(&self, _request: &Request, response: Response) -> Response {
        response
    }

    /// Writes one access-log line when access logging is enabled.
    fn log(&self, request: &Request, ctx: &RequestContext, status: StatusCode) {
        if !self.access_log {
            return;
        }
        let ip = Ipv6Addr::from(ctx.peer);
        let peer = match ip.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => ip.to_string(),
        };
        eprintln!(
            "{} {} {} {}",
            peer,
            request.method.as_str(),
            request.path(),
            status.code()
        );
    }
}

impl Service for App {
    fn handle(&self, request: &Request, ctx: &RequestContext) -> Response {
        // Rate limit before doing any routing or filesystem work.
        if let Some(limiter) = &self.rate_limiter {
            let decision = {
                let mut guard = limiter.lock().unwrap_or_else(|p| p.into_inner());
                guard.check(ctx.peer, ctx.now_unix_secs)
            };
            if let Decision::Deny { retry_after_secs } = decision {
                let response =
                    Response::text(StatusCode::TOO_MANY_REQUESTS, "429 Too Many Requests")
                        .with_header("Retry-After", &retry_after_secs.to_string());
                self.log(request, ctx, response.status);
                return response;
            }
        }

        let response = self.post_process(request, self.dispatch(request));
        self.log(request, ctx, response.status);
        response
    }
}

#[cfg(feature = "gzip")]
fn accepts_gzip(request: &Request) -> bool {
    request.header("accept-encoding").is_some_and(|value| {
        value
            .split(',')
            .any(|enc| matches!(enc.trim().split(';').next(), Some(name) if name.eq_ignore_ascii_case("gzip")))
    })
}

#[cfg(feature = "gzip")]
fn is_compressible(response: &Response) -> bool {
    match header_value(response, "content-type") {
        Some(ct) => {
            let ct = ct.to_ascii_lowercase();
            ct.starts_with("text/")
                || ct.starts_with("application/json")
                || ct.starts_with("application/xml")
                || ct.starts_with("application/javascript")
                || ct.starts_with("image/svg+xml")
        }
        None => false,
    }
}

#[cfg(feature = "gzip")]
fn header_value<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[cfg(feature = "gzip")]
fn has_header(response: &Response, name: &str) -> bool {
    header_value(response, name).is_some()
}

/// Builds an `Allow` header value from the methods registered for a path: the
/// registered methods, plus HEAD wherever GET is supported and OPTIONS (both
/// answerable here). Order is not significant (RFC 9110, Section 10.2.1).
fn allow_header_value(allowed: &[Method]) -> String {
    let mut methods: Vec<Method> = allowed.to_vec();
    if methods.contains(&Method::Get) && !methods.contains(&Method::Head) {
        methods.push(Method::Head);
    }
    if !methods.contains(&Method::Options) {
        methods.push(Method::Options);
    }
    let mut value = String::new();
    for (i, method) in methods.iter().enumerate() {
        if i > 0 {
            value.push_str(", ");
        }
        value.push_str(method.as_str());
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_header_synthesizes_head_and_options_for_get() {
        // A GET resource also answers HEAD and OPTIONS, so Allow must list them
        // even though only GET was registered; otherwise discovery understates
        // what the server actually serves.
        let allow = allow_header_value(&[Method::Get]);
        for method in ["GET", "HEAD", "OPTIONS"] {
            assert!(allow.contains(method), "Allow {allow:?} missing {method}");
        }
    }

    #[test]
    fn allow_header_omits_head_without_get() {
        // A QUERY-only resource does not answer HEAD; advertising it would claim
        // a method the server would reject.
        let allow = allow_header_value(&[Method::Query]);
        assert!(allow.contains("QUERY"));
        assert!(allow.contains("OPTIONS"));
        assert!(!allow.contains("HEAD"));
    }
}
