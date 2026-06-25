//! HTTP status codes and their reason phrases.

use core::fmt;

/// An HTTP status code paired with its standard reason phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusCode(u16);

impl StatusCode {
    pub const OK: StatusCode = StatusCode(200);
    pub const CREATED: StatusCode = StatusCode(201);
    pub const NO_CONTENT: StatusCode = StatusCode(204);
    pub const BAD_REQUEST: StatusCode = StatusCode(400);
    pub const UNAUTHORIZED: StatusCode = StatusCode(401);
    pub const FORBIDDEN: StatusCode = StatusCode(403);
    pub const NOT_FOUND: StatusCode = StatusCode(404);
    pub const METHOD_NOT_ALLOWED: StatusCode = StatusCode(405);
    pub const PAYLOAD_TOO_LARGE: StatusCode = StatusCode(413);
    pub const URI_TOO_LONG: StatusCode = StatusCode(414);
    pub const UNSUPPORTED_MEDIA_TYPE: StatusCode = StatusCode(415);
    pub const IM_A_TEAPOT: StatusCode = StatusCode(418);
    pub const TOO_MANY_REQUESTS: StatusCode = StatusCode(429);
    pub const REQUEST_HEADER_FIELDS_TOO_LARGE: StatusCode = StatusCode(431);
    pub const INTERNAL_SERVER_ERROR: StatusCode = StatusCode(500);
    pub const NOT_IMPLEMENTED: StatusCode = StatusCode(501);
    pub const HTTP_VERSION_NOT_SUPPORTED: StatusCode = StatusCode(505);

    /// Constructs a status from a raw code (for codes without a named constant).
    pub const fn new(code: u16) -> StatusCode {
        StatusCode(code)
    }

    /// The numeric status code.
    pub fn code(&self) -> u16 {
        self.0
    }

    /// The reason phrase, or an empty string for codes ferro does not name.
    pub fn reason(&self) -> &'static str {
        match self.0 {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Content Too Large",
            414 => "URI Too Long",
            415 => "Unsupported Media Type",
            418 => "I'm a teapot",
            429 => "Too Many Requests",
            431 => "Request Header Fields Too Large",
            500 => "Internal Server Error",
            501 => "Not Implemented",
            505 => "HTTP Version Not Supported",
            _ => "",
        }
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.0, self.reason())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_carry_reason_phrases() {
        assert_eq!(StatusCode::NOT_FOUND.code(), 404);
        assert_eq!(StatusCode::NOT_FOUND.reason(), "Not Found");
        assert_eq!(StatusCode::IM_A_TEAPOT.reason(), "I'm a teapot");
        assert_eq!(StatusCode::UNAUTHORIZED.code(), 401);
        assert_eq!(StatusCode::UNAUTHORIZED.reason(), "Unauthorized");
        assert_eq!(StatusCode::UNSUPPORTED_MEDIA_TYPE.code(), 415);
        assert_eq!(
            StatusCode::UNSUPPORTED_MEDIA_TYPE.reason(),
            "Unsupported Media Type"
        );
    }

    #[test]
    fn unnamed_codes_have_empty_reason() {
        // A code we do not name must still be representable, just without a phrase.
        assert_eq!(StatusCode::new(599).reason(), "");
    }
}
