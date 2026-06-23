//! A small, allocation-only `no_std` JSON parser for configuration files.
//!
//! Config input is local and trusted, so this favors clarity over raw speed.
//! It is still strict: it handles all string escapes (including `\uXXXX` and
//! surrogate pairs), bounds nesting depth, and rejects trailing data rather
//! than silently accepting partial input.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Maximum nesting depth of arrays and objects.
pub const MAX_DEPTH: usize = 64;

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    /// Object members, preserved in document order.
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    /// Looks up an object member by key, or `None` for non-objects/absent keys.
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        match self {
            JsonValue::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Returns the boolean value, if this is a `Bool`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the string value, if this is a `String`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns the array elements, if this is an `Array`.
    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    /// Returns a non-negative integer if this is a `Number` with no fractional
    /// part and within `u64` range. Used for ports, sizes, and durations.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            JsonValue::Number(n) => {
                let n = *n;
                // `fract`/`is_finite` are std-only float methods, unavailable in
                // the no_std core. Use comparisons (which are false for NaN/inf)
                // plus a truncate-and-round-trip check for integrality.
                if n >= 0.0 && n <= u64::MAX as f64 {
                    let truncated = n as u64;
                    if truncated as f64 == n {
                        return Some(truncated);
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// Why JSON parsing failed, with the byte offset where it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub kind: JsonErrorKind,
    pub offset: usize,
}

/// The category of a [`JsonError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonErrorKind {
    UnexpectedEnd,
    UnexpectedByte,
    InvalidNumber,
    InvalidString,
    InvalidEscape,
    InvalidUnicode,
    DepthExceeded,
    TrailingData,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self.kind {
            JsonErrorKind::UnexpectedEnd => "unexpected end of input",
            JsonErrorKind::UnexpectedByte => "unexpected byte",
            JsonErrorKind::InvalidNumber => "invalid number",
            JsonErrorKind::InvalidString => "invalid string",
            JsonErrorKind::InvalidEscape => "invalid escape sequence",
            JsonErrorKind::InvalidUnicode => "invalid \\u escape",
            JsonErrorKind::DepthExceeded => "nesting too deep",
            JsonErrorKind::TrailingData => "trailing data after value",
        };
        write!(f, "{} at byte {}", msg, self.offset)
    }
}

/// Parses a complete JSON document, rejecting any trailing non-whitespace.
pub fn parse(input: &str) -> Result<JsonValue, JsonError> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let value = p.parse_value(0)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(p.err(JsonErrorKind::TrailingData));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, kind: JsonErrorKind) -> JsonError {
        JsonError {
            kind,
            offset: self.pos,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        match self
            .peek()
            .ok_or_else(|| self.err(JsonErrorKind::UnexpectedEnd))?
        {
            b'{' => self.parse_object(depth),
            b'[' => self.parse_array(depth),
            b'"' => Ok(JsonValue::String(self.parse_string()?)),
            b't' | b'f' => self.parse_bool(),
            b'n' => self.parse_null(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(self.err(JsonErrorKind::UnexpectedByte)),
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), JsonError> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else if self.pos >= self.bytes.len() {
            Err(self.err(JsonErrorKind::UnexpectedEnd))
        } else {
            Err(self.err(JsonErrorKind::UnexpectedByte))
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        if depth + 1 > MAX_DEPTH {
            return Err(self.err(JsonErrorKind::DepthExceeded));
        }
        self.expect(b'{')?;
        let mut members = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(members));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err(JsonErrorKind::UnexpectedByte));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.parse_value(depth + 1)?;
            members.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(members));
                }
                Some(_) => return Err(self.err(JsonErrorKind::UnexpectedByte)),
                None => return Err(self.err(JsonErrorKind::UnexpectedEnd)),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        if depth + 1 > MAX_DEPTH {
            return Err(self.err(JsonErrorKind::DepthExceeded));
        }
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                Some(_) => return Err(self.err(JsonErrorKind::UnexpectedByte)),
                None => return Err(self.err(JsonErrorKind::UnexpectedEnd)),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let b = self
                .peek()
                .ok_or_else(|| self.err(JsonErrorKind::UnexpectedEnd))?;
            self.pos += 1;
            match b {
                b'"' => return Ok(out),
                b'\\' => self.parse_escape(&mut out)?,
                // Raw control characters are not allowed in JSON strings.
                0x00..=0x1F => return Err(self.err(JsonErrorKind::InvalidString)),
                _ => {
                    // Collect this UTF-8 sequence verbatim by decoding from the
                    // original bytes starting at the byte we just consumed.
                    let start = self.pos - 1;
                    let len = utf8_len(b);
                    if len == 0 || start + len > self.bytes.len() {
                        return Err(self.err(JsonErrorKind::InvalidString));
                    }
                    let chunk = &self.bytes[start..start + len];
                    let s = core::str::from_utf8(chunk)
                        .map_err(|_| self.err(JsonErrorKind::InvalidString))?;
                    out.push_str(s);
                    self.pos = start + len;
                }
            }
        }
    }

    fn parse_escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        let e = self
            .peek()
            .ok_or_else(|| self.err(JsonErrorKind::UnexpectedEnd))?;
        self.pos += 1;
        match e {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{0008}'),
            b'f' => out.push('\u{000C}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => self.parse_unicode_escape(out)?,
            _ => return Err(self.err(JsonErrorKind::InvalidEscape)),
        }
        Ok(())
    }

    fn parse_unicode_escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        let first = self.read_hex4()?;
        let scalar = if (0xD800..=0xDBFF).contains(&first) {
            // High surrogate: a low surrogate must immediately follow.
            if self.peek() != Some(b'\\') {
                return Err(self.err(JsonErrorKind::InvalidUnicode));
            }
            self.pos += 1;
            if self.peek() != Some(b'u') {
                return Err(self.err(JsonErrorKind::InvalidUnicode));
            }
            self.pos += 1;
            let low = self.read_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(self.err(JsonErrorKind::InvalidUnicode));
            }
            0x10000 + ((first as u32 - 0xD800) << 10) + (low as u32 - 0xDC00)
        } else if (0xDC00..=0xDFFF).contains(&first) {
            // A lone low surrogate is invalid.
            return Err(self.err(JsonErrorKind::InvalidUnicode));
        } else {
            first as u32
        };
        let ch = char::from_u32(scalar).ok_or_else(|| self.err(JsonErrorKind::InvalidUnicode))?;
        out.push(ch);
        Ok(())
    }

    fn read_hex4(&mut self) -> Result<u16, JsonError> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let b = self
                .peek()
                .ok_or_else(|| self.err(JsonErrorKind::UnexpectedEnd))?;
            let digit = (b as char)
                .to_digit(16)
                .ok_or_else(|| self.err(JsonErrorKind::InvalidUnicode))?;
            value = value * 16 + digit as u16;
            self.pos += 1;
        }
        Ok(value)
    }

    fn parse_bool(&mut self) -> Result<JsonValue, JsonError> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(JsonValue::Bool(true))
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(JsonValue::Bool(false))
        } else {
            Err(self.err(JsonErrorKind::UnexpectedByte))
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, JsonError> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(JsonValue::Null)
        } else {
            Err(self.err(JsonErrorKind::UnexpectedByte))
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        self.consume_digits()?;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            self.consume_digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.consume_digits()?;
        }
        let text = core::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err(JsonErrorKind::InvalidNumber))?;
        let n = text.parse::<f64>().map_err(|_| JsonError {
            kind: JsonErrorKind::InvalidNumber,
            offset: start,
        })?;
        Ok(JsonValue::Number(n))
    }

    fn consume_digits(&mut self) -> Result<(), JsonError> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err(JsonErrorKind::InvalidNumber));
        }
        Ok(())
    }
}

/// Length in bytes of a UTF-8 sequence given its leading byte (0 if invalid).
fn utf8_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn parses_nested_object() {
        let v = parse(r#"{"a": {"b": [1, 2, true]}, "c": null}"#).unwrap();
        let b = v.get("a").and_then(|a| a.get("b")).unwrap();
        assert_eq!(b.as_array().unwrap().len(), 3);
        assert_eq!(v.get("c"), Some(&JsonValue::Null));
    }

    #[test]
    fn parses_numbers_as_integers_when_whole() {
        let v = parse("1048576").unwrap();
        assert_eq!(v.as_u64(), Some(1_048_576));
        // A fractional number is not a valid u64.
        assert_eq!(parse("1.5").unwrap().as_u64(), None);
    }

    #[test]
    fn decodes_string_escapes() {
        let v = parse(r#""a\n\t\"\\\/b""#).unwrap();
        assert_eq!(v.as_str(), Some("a\n\t\"\\/b"));
    }

    #[test]
    fn decodes_basic_unicode_escape() {
        // U+00E7 (ç) written as ç.
        let v = parse(r#""ç""#).unwrap();
        assert_eq!(v.as_str(), Some("ç"));
    }

    #[test]
    fn decodes_surrogate_pair() {
        // U+1F600 GRINNING FACE as a surrogate pair.
        let v = parse(r#""😀""#).unwrap();
        assert_eq!(v.as_str(), Some("😀"));
    }

    #[test]
    fn rejects_lone_surrogate() {
        assert_eq!(
            parse(r#""\uD83D""#).unwrap_err().kind,
            JsonErrorKind::InvalidUnicode
        );
    }

    #[test]
    fn rejects_trailing_data() {
        assert_eq!(
            parse("{} {}").unwrap_err().kind,
            JsonErrorKind::TrailingData
        );
    }

    #[test]
    fn rejects_unterminated_string() {
        assert_eq!(
            parse(r#""abc"#).unwrap_err().kind,
            JsonErrorKind::UnexpectedEnd
        );
    }

    #[test]
    fn bounds_nesting_depth() {
        // Build an array nested deeper than MAX_DEPTH and confirm it is rejected
        // rather than overflowing the stack.
        let mut s = String::new();
        for _ in 0..(MAX_DEPTH + 2) {
            s.push('[');
        }
        assert_eq!(parse(&s).unwrap_err().kind, JsonErrorKind::DepthExceeded);
    }

    #[test]
    fn object_lookup_is_order_preserving_and_keyed() {
        let v = parse(r#"{"first": 1, "second": 2}"#).unwrap();
        assert_eq!(v.get("second").unwrap().as_u64(), Some(2));
        assert_eq!(v.get("missing"), None);
    }

    #[test]
    fn empty_containers() {
        assert_eq!(parse("{}").unwrap(), JsonValue::Object(Vec::new()));
        assert_eq!(parse("[]").unwrap(), JsonValue::Array(Vec::new()));
    }

    #[test]
    fn rejects_bare_keyword_typo() {
        assert!(parse("nul").is_err());
        assert_eq!(
            parse("tru").unwrap_err().kind,
            JsonErrorKind::UnexpectedByte
        );
    }

    #[test]
    fn error_carries_offset() {
        // The unexpected byte is the '}' closing an object expecting a value.
        let err = parse(r#"{"k":}"#).unwrap_err();
        assert_eq!(err.kind, JsonErrorKind::UnexpectedByte);
        assert_eq!(err.offset, 5);
    }

    #[test]
    fn string_to_string_roundtrip_helpers() {
        // Guard against as_str silently returning for non-strings.
        assert_eq!(parse("42").unwrap().as_str(), None);
        assert_eq!(
            parse(r#""x""#).unwrap().as_str(),
            Some("x".to_string().as_str())
        );
    }
}
