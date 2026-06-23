//! HTTP request methods.

use core::fmt;

/// An HTTP request method (RFC 7231 plus the common verbs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
    Connect,
    Trace,
}

impl Method {
    /// Parses a method from its ASCII token, returning `None` if unrecognized.
    pub fn from_bytes(token: &[u8]) -> Option<Method> {
        Some(match token {
            b"GET" => Method::Get,
            b"HEAD" => Method::Head,
            b"POST" => Method::Post,
            b"PUT" => Method::Put,
            b"PATCH" => Method::Patch,
            b"DELETE" => Method::Delete,
            b"OPTIONS" => Method::Options,
            b"CONNECT" => Method::Connect,
            b"TRACE" => Method::Trace,
            _ => return None,
        })
    }

    /// Returns the canonical uppercase token for this method.
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Options => "OPTIONS",
            Method::Connect => "CONNECT",
            Method::Trace => "TRACE",
        }
    }

    /// Whether a response to this method must omit its body (HEAD).
    pub fn is_head(&self) -> bool {
        matches!(self, Method::Head)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_methods() {
        assert_eq!(Method::from_bytes(b"GET"), Some(Method::Get));
        assert_eq!(Method::from_bytes(b"DELETE"), Some(Method::Delete));
    }

    #[test]
    fn rejects_unknown_and_is_case_sensitive() {
        // HTTP methods are case-sensitive tokens; "get" is not "GET".
        assert_eq!(Method::from_bytes(b"get"), None);
        assert_eq!(Method::from_bytes(b"BREW"), None);
    }

    #[test]
    fn only_head_suppresses_body() {
        assert!(Method::Head.is_head());
        assert!(!Method::Get.is_head());
    }
}
