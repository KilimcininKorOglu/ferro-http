//! The embedded-profile request handler: API router, then static files, then 404.
//!
//! This is the embedded counterpart of the std profile's `App`, minus the
//! std-only concerns (rate-limit mutex, gzip, access logging). It is generic
//! over any [`AssetSource`], so the bare-metal binary composes it over an
//! [`EmbeddedAssets`](ferro_core::asset::EmbeddedAssets) bundle while tests
//! compose it over an in-memory one.

use alloc::string::String;
use alloc::vec::Vec;

use ferro_core::asset::AssetSource;
use ferro_core::handler::static_files::serve_static;
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::router::Router;
use ferro_core::service::{RequestContext, Service};

/// Composes the API router with static-file serving over an [`AssetSource`].
///
/// On each request the router is tried first; for `GET`/`HEAD` a static asset is
/// served as a fallback; anything unmatched is a 404.
pub struct StaticRouter<A: AssetSource> {
    router: Router,
    assets: A,
    index_files: Vec<String>,
    mime_overrides: Vec<(String, String)>,
}

impl<A: AssetSource> StaticRouter<A> {
    /// Builds the handler from its parts.
    ///
    /// `index_files` are tried in order for directory-style requests;
    /// `mime_overrides` map file extensions to `Content-Type` values.
    pub fn new(
        router: Router,
        assets: A,
        index_files: Vec<String>,
        mime_overrides: Vec<(String, String)>,
    ) -> StaticRouter<A> {
        StaticRouter {
            router,
            assets,
            index_files,
            mime_overrides,
        }
    }
}

impl<A: AssetSource> Service for StaticRouter<A> {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use ferro_core::asset::EmbeddedAssets;
    use ferro_core::conn::{Connection, Step};
    use ferro_core::router::Params;

    static ASSETS: &[(&str, &[u8])] = &[("index.html", b"<h1>home</h1>")];

    fn ping(_req: &Request, _p: &Params) -> Response {
        Response::text(StatusCode::OK, "pong")
    }

    fn app() -> StaticRouter<EmbeddedAssets> {
        let mut router = Router::new();
        router.route(Method::Get, "/ping", ping);
        StaticRouter::new(
            router,
            EmbeddedAssets::new(ASSETS),
            Vec::from([String::from("index.html")]),
            Vec::new(),
        )
    }

    /// Runs one request end to end through a [`Connection`] and returns the wire.
    fn serve(app: &StaticRouter<EmbeddedAssets>, raw: &[u8]) -> String {
        let mut conn = Connection::new();
        conn.feed(raw);
        match conn.step(app, 0) {
            Step::Write { bytes, .. } => String::from_utf8(bytes).expect("utf-8"),
            Step::NeedMore => panic!("expected a response"),
        }
    }

    #[test]
    fn router_takes_precedence() {
        let wire = serve(&app(), b"GET /ping HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert!(wire.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(wire.ends_with("pong"));
    }

    #[test]
    fn static_asset_is_a_fallback() {
        let wire = serve(&app(), b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert!(wire.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(wire.ends_with("<h1>home</h1>"));
    }

    #[test]
    fn unknown_path_is_404() {
        let wire = serve(&app(), b"GET /nope HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert!(wire.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }
}
