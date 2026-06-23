//! Incremental, allocation-aware HTTP/1.1 request parser.
//!
//! [`parse`] is called repeatedly against a growing buffer. It returns
//! [`Parsed::Partial`] when more bytes are needed (consuming nothing), or
//! [`Parsed::Complete`] with the number of bytes the caller may drop from the
//! front of the buffer (so pipelined requests are supported). Malformed input
//! fails loudly with a [`ParseError`] rather than being silently coerced.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::http::method::Method;

/// Maximum size of the request line plus header block.
pub const MAX_HEAD_BYTES: usize = 8 * 1024;
/// Maximum number of header fields.
pub const MAX_HEADERS: usize = 64;
/// Maximum size of the request target (URI).
pub const MAX_TARGET_BYTES: usize = 8 * 1024;
/// Maximum size of a request body framed by `Content-Length`.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The HTTP protocol version of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    Http10,
    Http11,
}

/// A single request header field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// A fully parsed HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub target: String,
    pub version: Version,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

impl Request {
    /// Returns the first value of `name`, matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    /// The request path, with any query string stripped.
    pub fn path(&self) -> &str {
        match self.target.split_once('?') {
            Some((p, _)) => p,
            None => &self.target,
        }
    }

    /// Whether the connection should be kept alive after this request, per the
    /// `Connection` header with the HTTP-version default (keep-alive on 1.1).
    pub fn keep_alive(&self) -> bool {
        match self.header("connection") {
            Some(v) if v.eq_ignore_ascii_case("close") => false,
            Some(v) if v.eq_ignore_ascii_case("keep-alive") => true,
            _ => self.version == Version::Http11,
        }
    }
}

/// Why a request could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    HeadTooLarge,
    TooManyHeaders,
    MalformedRequestLine,
    UnknownMethod,
    TargetTooLong,
    UnsupportedVersion,
    MalformedHeader,
    BadContentLength,
    /// Both `Content-Length` and `Transfer-Encoding` present (request smuggling).
    ConflictingFraming,
    /// A `Transfer-Encoding` other than a lone `chunked` (not supported).
    UnsupportedTransferEncoding,
    /// A malformed chunk size, terminator, or trailer in a chunked body.
    MalformedChunk,
    BodyTooLarge,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            ParseError::HeadTooLarge => "request head too large",
            ParseError::TooManyHeaders => "too many headers",
            ParseError::MalformedRequestLine => "malformed request line",
            ParseError::UnknownMethod => "unknown method",
            ParseError::TargetTooLong => "request target too long",
            ParseError::UnsupportedVersion => "unsupported HTTP version",
            ParseError::MalformedHeader => "malformed header",
            ParseError::BadContentLength => "invalid Content-Length",
            ParseError::ConflictingFraming => "conflicting Content-Length and Transfer-Encoding",
            ParseError::UnsupportedTransferEncoding => "unsupported Transfer-Encoding",
            ParseError::MalformedChunk => "malformed chunked body",
            ParseError::BodyTooLarge => "request body too large",
        };
        f.write_str(msg)
    }
}

/// The outcome of a successful (non-error) parse attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// More bytes are required; nothing was consumed.
    Partial,
    /// One complete request; `consumed` bytes may be removed from the buffer.
    Complete { request: Request, consumed: usize },
}

/// Attempts to parse a single request from the front of `buf`.
pub fn parse(buf: &[u8]) -> Result<Parsed, ParseError> {
    let head_end = match find_subslice(buf, b"\r\n\r\n") {
        Some(i) => i + 4,
        None => {
            if buf.len() > MAX_HEAD_BYTES {
                return Err(ParseError::HeadTooLarge);
            }
            return Ok(Parsed::Partial);
        }
    };
    if head_end - 4 > MAX_HEAD_BYTES {
        return Err(ParseError::HeadTooLarge);
    }

    // Request line + header fields, without the terminating blank line.
    let mut lines = split_crlf(&buf[..head_end - 4]);
    let request_line = lines.next().ok_or(ParseError::MalformedRequestLine)?;
    let (method, target, version) = parse_request_line(request_line)?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            // A bare empty line inside the header block is malformed framing.
            return Err(ParseError::MalformedHeader);
        }
        if headers.len() >= MAX_HEADERS {
            return Err(ParseError::TooManyHeaders);
        }
        let (name, value) = parse_header_line(line)?;
        headers.push(Header { name, value });
    }

    let (body, consumed) = match body_framing(&headers)? {
        Framing::Fixed(len) => {
            if len > MAX_BODY_BYTES {
                return Err(ParseError::BodyTooLarge);
            }
            let total = head_end + len;
            if buf.len() < total {
                return Ok(Parsed::Partial);
            }
            (buf[head_end..total].to_vec(), total)
        }
        Framing::Chunked => match decode_chunked(&buf[head_end..])? {
            ChunkOutcome::Partial => return Ok(Parsed::Partial),
            ChunkOutcome::Complete { body, consumed } => (body, head_end + consumed),
        },
    };

    Ok(Parsed::Complete {
        request: Request {
            method,
            target,
            version,
            headers,
            body,
        },
        consumed,
    })
}

fn parse_request_line(line: &[u8]) -> Result<(Method, String, Version), ParseError> {
    let mut parts = line.splitn(3, |&b| b == b' ');
    let method_tok = parts.next().ok_or(ParseError::MalformedRequestLine)?;
    let target_tok = parts.next().ok_or(ParseError::MalformedRequestLine)?;
    let version_tok = parts.next().ok_or(ParseError::MalformedRequestLine)?;

    // A well-formed request line has exactly three tokens; a space left in the
    // version token means there were four or more.
    if version_tok.contains(&b' ') {
        return Err(ParseError::MalformedRequestLine);
    }

    let method = Method::from_bytes(method_tok).ok_or(ParseError::UnknownMethod)?;

    if target_tok.len() > MAX_TARGET_BYTES {
        return Err(ParseError::TargetTooLong);
    }
    let target = core::str::from_utf8(target_tok).map_err(|_| ParseError::MalformedRequestLine)?;
    if target.is_empty() {
        return Err(ParseError::MalformedRequestLine);
    }

    let version = match version_tok {
        b"HTTP/1.1" => Version::Http11,
        b"HTTP/1.0" => Version::Http10,
        _ => return Err(ParseError::UnsupportedVersion),
    };

    Ok((method, target.to_string(), version))
}

fn parse_header_line(line: &[u8]) -> Result<(String, String), ParseError> {
    let colon = line
        .iter()
        .position(|&b| b == b':')
        .ok_or(ParseError::MalformedHeader)?;

    let name = &line[..colon];
    if name.is_empty() || !name.iter().all(|&b| is_token_byte(b)) {
        return Err(ParseError::MalformedHeader);
    }

    // Trim optional leading/trailing whitespace (OWS) from the value.
    let mut value = &line[colon + 1..];
    while let [first, rest @ ..] = value {
        if *first == b' ' || *first == b'\t' {
            value = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = value {
        if *last == b' ' || *last == b'\t' {
            value = rest;
        } else {
            break;
        }
    }

    let name = core::str::from_utf8(name).map_err(|_| ParseError::MalformedHeader)?;
    let value = core::str::from_utf8(value).map_err(|_| ParseError::MalformedHeader)?;
    Ok((name.to_string(), value.to_string()))
}

/// How the request body is framed.
enum Framing {
    /// A body of exactly this many bytes (`Content-Length`, possibly zero).
    Fixed(usize),
    /// A chunked body to be decoded.
    Chunked,
}

fn body_framing(headers: &[Header]) -> Result<Framing, ParseError> {
    let transfer_encodings: Vec<&Header> = headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("transfer-encoding"))
        .collect();
    let content_lengths: Vec<&Header> = headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("content-length"))
        .collect();

    if !transfer_encodings.is_empty() && !content_lengths.is_empty() {
        return Err(ParseError::ConflictingFraming);
    }
    if !transfer_encodings.is_empty() {
        // Only a single `Transfer-Encoding: chunked` is supported; multiple TE
        // headers or any other coding is rejected rather than mis-framed.
        if transfer_encodings.len() == 1
            && transfer_encodings[0]
                .value
                .trim()
                .eq_ignore_ascii_case("chunked")
        {
            return Ok(Framing::Chunked);
        }
        return Err(ParseError::UnsupportedTransferEncoding);
    }
    if content_lengths.len() > 1 {
        return Err(ParseError::BadContentLength);
    }
    if let Some(h) = content_lengths.first() {
        let raw = h.value.trim();
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError::BadContentLength);
        }
        let len = raw
            .parse::<usize>()
            .map_err(|_| ParseError::BadContentLength)?;
        return Ok(Framing::Fixed(len));
    }
    Ok(Framing::Fixed(0))
}

/// Outcome of decoding a chunked body from the bytes after the header block.
enum ChunkOutcome {
    Partial,
    Complete { body: Vec<u8>, consumed: usize },
}

/// Decodes a chunked transfer body. `buf` begins at the first chunk size line.
/// Chunk extensions and trailers are skipped; their content is not retained.
fn decode_chunked(buf: &[u8]) -> Result<ChunkOutcome, ParseError> {
    let mut pos = 0;
    let mut body = Vec::new();
    loop {
        let line_len = match find_subslice(&buf[pos..], b"\r\n") {
            Some(i) => i,
            None => return Ok(ChunkOutcome::Partial),
        };
        let size_line = &buf[pos..pos + line_len];
        // Drop any chunk extensions following a ';'.
        let size_tok = match size_line.iter().position(|&b| b == b';') {
            Some(i) => &size_line[..i],
            None => size_line,
        };
        let size = parse_hex(size_tok)?;
        let data_start = pos + line_len + 2;

        if size == 0 {
            // Final chunk: skip optional trailers up to the terminating blank line.
            let mut tpos = data_start;
            loop {
                let trailer_len = match find_subslice(&buf[tpos..], b"\r\n") {
                    Some(i) => i,
                    None => return Ok(ChunkOutcome::Partial),
                };
                if trailer_len == 0 {
                    return Ok(ChunkOutcome::Complete {
                        body,
                        consumed: tpos + 2,
                    });
                }
                tpos += trailer_len + 2;
            }
        }

        let data_end = data_start + size;
        if buf.len() < data_end + 2 {
            return Ok(ChunkOutcome::Partial);
        }
        if &buf[data_end..data_end + 2] != b"\r\n" {
            return Err(ParseError::MalformedChunk);
        }
        body.extend_from_slice(&buf[data_start..data_end]);
        if body.len() > MAX_BODY_BYTES {
            return Err(ParseError::BodyTooLarge);
        }
        pos = data_end + 2;
    }
}

/// Parses a hexadecimal chunk size, rejecting empty or overflowing values.
fn parse_hex(bytes: &[u8]) -> Result<usize, ParseError> {
    if bytes.is_empty() {
        return Err(ParseError::MalformedChunk);
    }
    let mut value: usize = 0;
    for &b in bytes {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(ParseError::MalformedChunk),
        };
        value = value
            .checked_mul(16)
            .and_then(|v| v.checked_add(digit as usize))
            .ok_or(ParseError::MalformedChunk)?;
    }
    Ok(value)
}

/// RFC 7230 token characters (valid in a header field name).
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// Returns the index of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Splits a byte block into lines on CRLF, yielding each line without the CRLF.
fn split_crlf(block: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = block;
    let mut done = false;
    core::iter::from_fn(move || {
        if done {
            return None;
        }
        match find_subslice(rest, b"\r\n") {
            Some(i) => {
                let line = &rest[..i];
                rest = &rest[i + 2..];
                Some(line)
            }
            None => {
                done = true;
                Some(rest)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(buf: &[u8]) -> (Request, usize) {
        match parse(buf).unwrap() {
            Parsed::Complete { request, consumed } => (request, consumed),
            Parsed::Partial => panic!("expected complete, got partial"),
        }
    }

    #[test]
    fn partial_until_head_terminator() {
        // Without the blank line, the parser must ask for more, not guess.
        assert!(matches!(
            parse(b"GET / HTTP/1.1\r\nHost: x\r\n").unwrap(),
            Parsed::Partial
        ));
    }

    #[test]
    fn parses_simple_get() {
        let (req, consumed) = complete(b"GET /index.html HTTP/1.1\r\nHost: example.com\r\n\r\n");
        assert_eq!(req.method, Method::Get);
        assert_eq!(req.target, "/index.html");
        assert_eq!(req.version, Version::Http11);
        assert_eq!(req.header("host"), Some("example.com"));
        assert_eq!(consumed, 47);
        assert!(req.body.is_empty());
    }

    #[test]
    fn path_strips_query_string() {
        let (req, _) = complete(b"GET /search?q=rust HTTP/1.1\r\n\r\n");
        assert_eq!(req.path(), "/search");
        assert_eq!(req.target, "/search?q=rust");
    }

    #[test]
    fn keep_alive_follows_version_and_header() {
        // HTTP/1.1 defaults to keep-alive; an explicit close overrides it.
        let (a, _) = complete(b"GET / HTTP/1.1\r\n\r\n");
        assert!(a.keep_alive());
        let (b, _) = complete(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert!(!b.keep_alive());
        // HTTP/1.0 defaults to close unless it asks to keep alive.
        let (c, _) = complete(b"GET / HTTP/1.0\r\n\r\n");
        assert!(!c.keep_alive());
        let (d, _) = complete(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n");
        assert!(d.keep_alive());
    }

    #[test]
    fn body_framed_by_content_length() {
        let raw = b"POST /f HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let (req, consumed) = complete(raw);
        assert_eq!(req.body, b"hello");
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn partial_when_body_incomplete() {
        // Head complete, but the declared body has not fully arrived yet.
        assert!(matches!(
            parse(b"POST /f HTTP/1.1\r\nContent-Length: 5\r\n\r\nhel").unwrap(),
            Parsed::Partial
        ));
    }

    #[test]
    fn rejects_unknown_method() {
        assert_eq!(
            parse(b"BREW / HTTP/1.1\r\n\r\n"),
            Err(ParseError::UnknownMethod)
        );
    }

    #[test]
    fn rejects_unsupported_version() {
        assert_eq!(
            parse(b"GET / HTTP/2.0\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
    }

    #[test]
    fn rejects_smuggling_framing() {
        // Both framing headers together is the classic request-smuggling vector.
        let raw =
            b"POST / HTTP/1.1\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\nhello";
        assert_eq!(parse(raw), Err(ParseError::ConflictingFraming));
    }

    #[test]
    fn decodes_chunked_body() {
        // "Wikipedia" in two chunks, then the terminating zero chunk.
        let raw = b"POST /f HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
                    4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let (req, consumed) = complete(raw);
        assert_eq!(req.body, b"Wikipedia");
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn chunked_body_is_partial_until_terminator() {
        // The zero chunk and final CRLF have not arrived yet.
        let raw = b"POST /f HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n";
        assert!(matches!(parse(raw).unwrap(), Parsed::Partial));
    }

    #[test]
    fn rejects_malformed_chunk_size() {
        let raw = b"POST /f HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\nWiki\r\n0\r\n\r\n";
        assert_eq!(parse(raw), Err(ParseError::MalformedChunk));
    }

    #[test]
    fn rejects_non_chunked_transfer_encoding() {
        let raw = b"POST /f HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n";
        assert_eq!(parse(raw), Err(ParseError::UnsupportedTransferEncoding));
    }

    #[test]
    fn rejects_bad_content_length() {
        let raw = b"POST / HTTP/1.1\r\nContent-Length: 1x\r\n\r\n";
        assert_eq!(parse(raw), Err(ParseError::BadContentLength));
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let raw = b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx";
        assert_eq!(parse(raw), Err(ParseError::BadContentLength));
    }

    #[test]
    fn pipelined_consumed_count_allows_next_request() {
        // Two requests back to back: the first parse consumes only its own bytes.
        let raw = b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        let (first, consumed) = complete(raw);
        assert_eq!(first.target, "/a");
        let (second, _) = complete(&raw[consumed..]);
        assert_eq!(second.target, "/b");
    }
}
