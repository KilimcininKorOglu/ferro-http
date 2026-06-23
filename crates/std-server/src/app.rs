//! The std-profile request handler: rate limit, API router, static files, 404.

use std::net::Ipv6Addr;
use std::sync::Mutex;

use ferro_core::handler::static_files::serve_static;
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::router::Router;
use ferro_core::security::{Decision, RateLimiter};
use ferro_core::service::{RequestContext, Service};

use crate::fs_assets::FsAssets;

/// Composes per-peer rate limiting, the API router, and filesystem static
/// serving, with optional access logging.
pub struct App {
    router: Router,
    assets: FsAssets,
    index_files: Vec<String>,
    mime_overrides: Vec<(String, String)>,
    rate_limiter: Option<Mutex<RateLimiter>>,
    access_log: bool,
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
    ) -> App {
        App {
            router,
            assets,
            index_files,
            mime_overrides,
            rate_limiter: rate_limiter.map(Mutex::new),
            access_log,
        }
    }

    /// API routes first, then static files (GET/HEAD), then 404.
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
        Response::text(StatusCode::NOT_FOUND, "404 Not Found")
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

        let response = self.dispatch(request);
        self.log(request, ctx, response.status);
        response
    }
}
