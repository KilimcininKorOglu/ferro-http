//! The std-profile request handler: API router, then static files, then 404.

use ferro_core::handler::static_files::serve_static;
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::router::Router;
use ferro_core::service::Service;

use crate::fs_assets::FsAssets;

/// Composes the API router with filesystem static serving.
pub struct App {
    router: Router,
    assets: FsAssets,
    index_files: Vec<String>,
    mime_overrides: Vec<(String, String)>,
}

impl App {
    /// Builds the application from its parts.
    pub fn new(
        router: Router,
        assets: FsAssets,
        index_files: Vec<String>,
        mime_overrides: Vec<(String, String)>,
    ) -> App {
        App {
            router,
            assets,
            index_files,
            mime_overrides,
        }
    }
}

impl Service for App {
    fn handle(&self, request: &Request) -> Response {
        // API routes win first.
        if let Some(response) = self.router.dispatch(request) {
            return response;
        }
        // Then static files, for body-bearing read methods (GET and HEAD).
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
