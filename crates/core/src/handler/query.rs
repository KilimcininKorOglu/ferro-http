//! RFC 10008 QUERY method helpers.

use alloc::string::String;

use crate::http::request::Request;
use crate::http::response::Response;
use crate::http::status::StatusCode;

/// Validates the `Content-Type` of a QUERY request against the media types a
/// resource accepts (RFC 10008, Section 2).
///
/// QUERY semantics come entirely from the request content and its media type, so
/// the server must reject a request whose type is absent or unsupported:
///
/// - a missing `Content-Type` yields `400 Bad Request`;
/// - a type not listed in `accepted` yields `415 Unsupported Media Type`,
///   advertising the supported types in the `Accept-Query` response field
///   (RFC 10008, Section 3).
///
/// Returns `Ok(())` when the type is acceptable, otherwise the error response for
/// the handler to return. Media type parameters (such as `; charset=utf-8`) are
/// ignored and the comparison is case-insensitive.
pub fn query_content_type_check(request: &Request, accepted: &[&str]) -> Result<(), Response> {
    let content_type = match request.header("content-type") {
        Some(value) => value,
        None => {
            return Err(Response::text(
                StatusCode::BAD_REQUEST,
                "QUERY requests must carry a Content-Type",
            ));
        }
    };

    // Match on the bare media type (type/subtype), ignoring any parameters.
    let media_type = content_type.split(';').next().unwrap_or("").trim();
    if accepted.iter().any(|a| a.eq_ignore_ascii_case(media_type)) {
        return Ok(());
    }

    Err(Response::text(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "Unsupported query media type",
    )
    .with_header("Accept-Query", &join_media_types(accepted)))
}

/// Joins media types into an `Accept-Query` value: a comma-separated list of
/// media ranges (RFC 10008, Section 3; Structured Fields list per RFC 9651).
fn join_media_types(types: &[&str]) -> String {
    let mut out = String::new();
    for (i, t) in types.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{parse, Parsed};

    const ACCEPTED: &[&str] = &["application/x-www-form-urlencoded", "application/sql"];

    fn req(raw: &[u8]) -> Request {
        match parse(raw).unwrap() {
            Parsed::Complete { request, .. } => request,
            Parsed::Partial => panic!("expected a complete request"),
        }
    }

    #[test]
    fn missing_content_type_is_a_400() {
        // RFC 10008 Section 2: QUERY semantics depend on the media type, so a
        // request without one cannot be interpreted and must be refused, not
        // guessed at from the body.
        let r = req(b"QUERY /s HTTP/1.1\r\nContent-Length: 1\r\n\r\nx");
        let err = query_content_type_check(&r, ACCEPTED).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn unsupported_type_is_a_415_advertising_accept_query() {
        // A 415 must steer the client to a usable format via Accept-Query rather
        // than failing opaquely; that discovery is the point of the field.
        let r = req(
            b"QUERY /s HTTP/1.1\r\nContent-Type: application/xml\r\nContent-Length: 1\r\n\r\nx",
        );
        let err = query_content_type_check(&r, ACCEPTED).unwrap_err();
        assert_eq!(err.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let accept_query = err
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("accept-query"))
            .map(|(_, value)| value.as_str());
        assert_eq!(
            accept_query,
            Some("application/x-www-form-urlencoded, application/sql")
        );
    }

    #[test]
    fn supported_type_passes_ignoring_parameters() {
        // A charset parameter must not defeat the match: per RFC 10008 the bare
        // media type carries the QUERY semantics, parameters are metadata.
        let r = req(
            b"QUERY /s HTTP/1.1\r\nContent-Type: application/sql; charset=utf-8\r\nContent-Length: 1\r\n\r\nx",
        );
        assert!(query_content_type_check(&r, ACCEPTED).is_ok());
    }
}
