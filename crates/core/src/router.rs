//! Minimal pattern router: static and `:param` path segments.
//!
//! Handlers are plain function pointers so the router stays `no_std` and
//! allocation-light. This is the Faz 1 skeleton for the simple API router in
//! the v1 scope; middleware and richer matching come later.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::http::method::Method;
use crate::http::request::Request;
use crate::http::response::Response;
use crate::http::status::StatusCode;
use crate::service::{RequestContext, Service};

/// Path parameters captured from `:name` segments during matching.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Params {
    pairs: Vec<(String, String)>,
}

impl Params {
    /// Returns the captured value for parameter `name`, if present.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A route handler: it receives the request and any captured path params.
pub type Handler = fn(&Request, &Params) -> Response;

enum Segment {
    Static(String),
    Param(String),
}

struct Route {
    method: Method,
    segments: Vec<Segment>,
    handler: Handler,
}

/// A table of routes matched in registration order.
#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    /// Creates an empty router.
    pub fn new() -> Router {
        Router { routes: Vec::new() }
    }

    /// Registers `handler` for `method` requests whose path matches `pattern`.
    /// Pattern segments beginning with `:` capture that path segment by name.
    pub fn route(&mut self, method: Method, pattern: &str, handler: Handler) -> &mut Router {
        self.routes.push(Route {
            method,
            segments: parse_pattern(pattern),
            handler,
        });
        self
    }

    /// Finds the first matching route and runs its handler, or returns `None`
    /// when no route matches (the caller then produces a 404). A HEAD request is
    /// answered by a matching GET route (its body is dropped at serialization).
    pub fn dispatch(&self, request: &Request) -> Option<Response> {
        let path = request.path();
        for route in &self.routes {
            if !method_matches(route.method, request.method) {
                continue;
            }
            if let Some(params) = match_segments(&route.segments, path) {
                return Some((route.handler)(request, &params));
            }
        }
        None
    }

    /// Returns the methods registered for routes whose pattern matches `path`,
    /// in registration order and de-duplicated. The list is empty when no route
    /// matches the path (i.e. the path is not a router resource). Callers use it
    /// to build an `Allow` header and to answer OPTIONS or 405 for known paths.
    pub fn allowed_methods(&self, path: &str) -> Vec<Method> {
        let mut methods: Vec<Method> = Vec::new();
        for route in &self.routes {
            if match_segments(&route.segments, path).is_some() && !methods.contains(&route.method) {
                methods.push(route.method);
            }
        }
        methods
    }
}

/// Whether a route registered for `route_method` answers a `request_method`
/// request. HEAD is answered by a GET route, since HEAD is GET without a body
/// (RFC 9110, Section 9.3.2); the body is dropped at serialization time.
fn method_matches(route_method: Method, request_method: Method) -> bool {
    route_method == request_method
        || (request_method == Method::Head && route_method == Method::Get)
}

impl Service for Router {
    /// Dispatches to a matching route, or returns 404 when none matches. A
    /// profile that also serves static files composes its own [`Service`] that
    /// falls through to the filesystem before this 404.
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        self.dispatch(request)
            .unwrap_or_else(|| Response::text(StatusCode::NOT_FOUND, "404 Not Found"))
    }
}

fn parse_pattern(pattern: &str) -> Vec<Segment> {
    split_path(pattern)
        .map(|seg| {
            if let Some(name) = seg.strip_prefix(':') {
                Segment::Param(name.to_string())
            } else {
                Segment::Static(seg.to_string())
            }
        })
        .collect()
}

fn match_segments(pattern: &[Segment], path: &str) -> Option<Params> {
    let actual: Vec<&str> = split_path(path).collect();
    if actual.len() != pattern.len() {
        return None;
    }
    let mut params = Params::default();
    for (seg, value) in pattern.iter().zip(actual.iter()) {
        match seg {
            Segment::Static(s) => {
                if s != value {
                    return None;
                }
            }
            Segment::Param(name) => {
                params.pairs.push((name.clone(), (*value).to_string()));
            }
        }
    }
    Some(params)
}

/// Splits a path into non-empty segments, ignoring leading, trailing, and
/// duplicate slashes (so `/api/users/` matches `/api/users`).
fn split_path(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::status::StatusCode;

    fn make_request(method: Method, target: &str) -> Request {
        Request {
            method,
            target: target.to_string(),
            version: crate::http::request::Version::Http11,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn users(_req: &Request, _p: &Params) -> Response {
        Response::json(StatusCode::OK, "{\"users\":[]}")
    }

    fn one_user(_req: &Request, p: &Params) -> Response {
        // Echo the captured id so the test can prove the param actually bound.
        Response::text(StatusCode::OK, p.get("id").unwrap_or("none"))
    }

    fn router() -> Router {
        let mut r = Router::new();
        r.route(Method::Get, "/api/users", users);
        r.route(Method::Get, "/api/users/:id", one_user);
        r
    }

    #[test]
    fn matches_static_route() {
        let resp = router()
            .dispatch(&make_request(Method::Get, "/api/users"))
            .expect("route should match");
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(resp.body, b"{\"users\":[]}");
    }

    #[test]
    fn captures_path_parameter() {
        let resp = router()
            .dispatch(&make_request(Method::Get, "/api/users/42"))
            .expect("param route should match");
        assert_eq!(resp.body, b"42");
    }

    #[test]
    fn trailing_slash_is_ignored() {
        assert!(router()
            .dispatch(&make_request(Method::Get, "/api/users/"))
            .is_some());
    }

    #[test]
    fn no_match_returns_none() {
        assert!(router()
            .dispatch(&make_request(Method::Get, "/nope"))
            .is_none());
    }

    #[test]
    fn method_must_match() {
        // Same path, wrong method: the GET routes must not answer a POST.
        assert!(router()
            .dispatch(&make_request(Method::Post, "/api/users"))
            .is_none());
    }

    #[test]
    fn head_is_answered_by_a_get_route() {
        // HEAD must reach a GET handler (the body is dropped at serialization),
        // otherwise HEAD on an API resource would wrongly 404.
        let resp = router()
            .dispatch(&make_request(Method::Head, "/api/users"))
            .expect("HEAD should be served by the GET route");
        assert_eq!(resp.status, StatusCode::OK);
    }

    #[test]
    fn allowed_methods_lists_registered_methods_for_a_path() {
        // A path registered under several methods must report all of them so the
        // caller can build a truthful Allow header; an unknown path reports none.
        let mut r = router();
        r.route(Method::Query, "/api/users", users);
        let allowed = r.allowed_methods("/api/users");
        assert_eq!(allowed.len(), 2);
        assert!(allowed.contains(&Method::Get));
        assert!(allowed.contains(&Method::Query));
        assert!(r.allowed_methods("/nope").is_empty());
    }
}
