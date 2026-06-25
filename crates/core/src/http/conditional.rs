//! RFC 9110 conditional-request (precondition) and Range evaluation, applied to
//! a freshly built static `200 OK` response.

use alloc::format;

use crate::http::date::parse_http_date;
use crate::http::request::Request;
use crate::http::response::Response;
use crate::http::status::StatusCode;

/// Applies preconditions and Range to a `200 OK` static response.
///
/// Preconditions win: when `If-None-Match`/`If-Modified-Since` show the client's
/// cached copy is still current, the body is replaced with `304 Not Modified`
/// carrying just the validators (RFC 9110 13.1, 15.4.5). Otherwise a `Range`
/// request is served as `206 Partial Content`, or `416 Range Not Satisfiable`
/// when the range cannot be met (RFC 9110 14). A non-200 response, or one
/// without validators, is returned unchanged.
pub fn evaluate(request: &Request, response: Response) -> Response {
    if response.status != StatusCode::OK {
        return response;
    }
    if not_modified(request, &response) {
        return to_not_modified(&response);
    }
    apply_range(request, response)
}

/// Whether the client's validators show its cached copy is still current.
/// `If-None-Match` takes precedence over `If-Modified-Since` (RFC 9110 13.1.3).
fn not_modified(request: &Request, response: &Response) -> bool {
    if let Some(inm) = request.header("if-none-match") {
        let etag = header(response, "etag");
        return inm.trim() == "*"
            || etag.is_some_and(|tag| inm.split(',').any(|c| weak_eq(c.trim(), tag)));
    }
    if let (Some(ims), Some(lm)) = (
        request.header("if-modified-since"),
        header(response, "last-modified"),
    ) {
        if let (Some(ims_secs), Some(lm_secs)) = (parse_http_date(ims), parse_http_date(lm)) {
            return lm_secs <= ims_secs;
        }
    }
    false
}

/// Weak entity-tag comparison (RFC 9110 8.8.3.2): the `W/` weakness prefix is
/// ignored on either side.
fn weak_eq(a: &str, b: &str) -> bool {
    a.strip_prefix("W/").unwrap_or(a) == b.strip_prefix("W/").unwrap_or(b)
}

/// Builds the `304 Not Modified` response, keeping only the metadata a 304 may
/// carry (RFC 9110 15.4.5) and no body.
fn to_not_modified(response: &Response) -> Response {
    let mut not_modified = Response::new(StatusCode::NOT_MODIFIED);
    for name in ["ETag", "Last-Modified", "Cache-Control", "Vary"] {
        if let Some(value) = header(response, name) {
            not_modified = not_modified.with_header(name, value);
        }
    }
    not_modified
}

/// Serves a single `Range` as `206`, returns `416` when unsatisfiable, or the
/// full response when there is no range, the range is unparsable, or more than
/// one range is requested (a server MAY ignore Range, RFC 9110 14.2).
fn apply_range(request: &Request, response: Response) -> Response {
    let range = match request.header("range") {
        Some(r) => r,
        None => return response,
    };
    let len = response.body.len() as u64;
    match parse_single_range(range, len) {
        Some(RangeOutcome::Satisfiable { start, end }) => {
            let mut partial = Response::new(StatusCode::PARTIAL_CONTENT)
                .with_header("Accept-Ranges", "bytes")
                .with_header("Content-Range", &format!("bytes {start}-{end}/{len}"));
            for name in ["Content-Type", "ETag", "Last-Modified"] {
                if let Some(value) = header(&response, name) {
                    partial = partial.with_header(name, value);
                }
            }
            partial.body = response.body[start as usize..=end as usize].to_vec();
            partial
        }
        Some(RangeOutcome::Unsatisfiable) => Response::new(StatusCode::RANGE_NOT_SATISFIABLE)
            .with_header("Content-Range", &format!("bytes */{len}")),
        None => response,
    }
}

/// The outcome of resolving a single byte range against a known length.
enum RangeOutcome {
    Satisfiable { start: u64, end: u64 },
    Unsatisfiable,
}

/// Parses a single `bytes=` range against `len`. Returns `None` for an
/// unparsable spec or a multi-range request (the caller then serves the full
/// body), per RFC 9110 14.1.1.
fn parse_single_range(value: &str, len: u64) -> Option<RangeOutcome> {
    let spec = value.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    let (first, last) = (first.trim(), last.trim());

    if first.is_empty() {
        // Suffix range "-N": the last N bytes.
        let n: u64 = last.parse().ok()?;
        if n == 0 || len == 0 {
            return Some(RangeOutcome::Unsatisfiable);
        }
        let n = n.min(len);
        return Some(RangeOutcome::Satisfiable {
            start: len - n,
            end: len - 1,
        });
    }

    let start: u64 = first.parse().ok()?;
    if start >= len {
        return Some(RangeOutcome::Unsatisfiable);
    }
    let end = if last.is_empty() {
        len - 1
    } else {
        let requested: u64 = last.parse().ok()?;
        if requested < start {
            return Some(RangeOutcome::Unsatisfiable);
        }
        requested.min(len - 1)
    };
    Some(RangeOutcome::Satisfiable { start, end })
}

/// Case-insensitive lookup of a response header value.
fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{parse, Parsed};

    fn req(raw: &[u8]) -> Request {
        match parse(raw).unwrap() {
            Parsed::Complete { request, .. } => request,
            Parsed::Partial => panic!("expected a complete request"),
        }
    }

    /// A 200 response with body, ETag, and Last-Modified, as serve_static builds.
    fn full() -> Response {
        let mut r = Response::new(StatusCode::OK)
            .with_header("Content-Type", "text/plain")
            .with_header("Accept-Ranges", "bytes")
            .with_header("ETag", "\"6-aabb\"")
            .with_header("Last-Modified", "Sat, 29 Feb 2020 00:00:00 GMT");
        r.body = b"0123456789".to_vec();
        r
    }

    #[test]
    fn matching_if_none_match_yields_304_without_body() {
        // The client's cached ETag matches, so revalidation returns a bodyless 304
        // that still carries the validator.
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nIf-None-Match: \"6-aabb\"\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::NOT_MODIFIED);
        assert!(out.body.is_empty());
        assert!(out
            .headers
            .iter()
            .any(|(n, v)| n == "ETag" && v == "\"6-aabb\""));
    }

    #[test]
    fn star_if_none_match_yields_304() {
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nIf-None-Match: *\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn non_matching_if_none_match_serves_full() {
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nIf-None-Match: \"other\"\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::OK);
        assert_eq!(out.body, b"0123456789");
    }

    #[test]
    fn if_modified_since_after_mtime_yields_304() {
        // The client's copy is at least as new as Last-Modified, so 304.
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nIf-Modified-Since: Sat, 29 Feb 2020 00:00:00 GMT\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn if_modified_since_before_mtime_serves_full() {
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nIf-Modified-Since: Thu, 01 Jan 1970 00:00:00 GMT\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::OK);
    }

    #[test]
    fn range_yields_206_with_content_range_and_sliced_body() {
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nRange: bytes=2-5\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(out.body, b"2345");
        assert!(out
            .headers
            .iter()
            .any(|(n, v)| n == "Content-Range" && v == "bytes 2-5/10"));
    }

    #[test]
    fn suffix_range_serves_last_bytes() {
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nRange: bytes=-3\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(out.body, b"789");
        assert!(out
            .headers
            .iter()
            .any(|(n, v)| n == "Content-Range" && v == "bytes 7-9/10"));
    }

    #[test]
    fn unsatisfiable_range_yields_416() {
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nRange: bytes=50-60\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(out
            .headers
            .iter()
            .any(|(n, v)| n == "Content-Range" && v == "bytes */10"));
    }

    #[test]
    fn multi_range_falls_back_to_full_200() {
        // Multipart byteranges are unsupported, so the full body is served (allowed).
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nRange: bytes=0-1,3-4\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::OK);
        assert_eq!(out.body, b"0123456789");
    }

    #[test]
    fn precondition_takes_precedence_over_range() {
        // When the cache is current, a 304 wins even if a Range was also sent.
        let out = evaluate(
            &req(b"GET /f HTTP/1.1\r\nHost: h\r\nIf-None-Match: \"6-aabb\"\r\nRange: bytes=0-1\r\n\r\n"),
            full(),
        );
        assert_eq!(out.status, StatusCode::NOT_MODIFIED);
    }
}
