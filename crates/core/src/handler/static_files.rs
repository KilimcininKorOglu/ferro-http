//! Static file serving over an [`AssetSource`], with lexical path safety.
//!
//! Path safety here is purely lexical (percent-decode, then reject `..`,
//! backslashes, and control bytes). Canonical-prefix and symlink hardening,
//! which need the filesystem, live in the std `AssetSource` and a later phase.

use alloc::string::String;
use alloc::vec::Vec;

use crate::asset::AssetSource;
use crate::http::response::Response;
use crate::http::status::StatusCode;

/// Serves the static asset addressed by `path` (a request path, query already
/// stripped). Returns `None` when the path is unsafe or no asset matches, so
/// the caller produces a 404 without leaking which case occurred.
///
/// For a directory-style request (root, or a trailing slash) the `index_files`
/// are tried in order. The response carries a guessed `Content-Type`; HEAD is
/// handled by the caller dropping the body at serialization time.
pub fn serve_static<A: AssetSource>(
    path: &str,
    index_files: &[String],
    assets: &A,
) -> Option<Response> {
    let decoded = percent_decode(path)?;
    let segments = sanitize(&decoded)?;
    let is_directory = segments.is_empty() || decoded.ends_with('/');

    if is_directory {
        let base = segments.join("/");
        for index in index_files {
            let candidate = if base.is_empty() {
                index.clone()
            } else {
                let mut c = base.clone();
                c.push('/');
                c.push_str(index);
                c
            };
            if let Some(asset) = assets.load(&candidate) {
                return Some(file_response(&candidate, asset.bytes));
            }
        }
        None
    } else {
        let rel = segments.join("/");
        assets
            .load(&rel)
            .map(|asset| file_response(&rel, asset.bytes))
    }
}

fn file_response(name: &str, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(StatusCode::OK).with_header("Content-Type", guess_mime(name));
    response.body = bytes;
    response
}

/// Splits a decoded path into safe segments, or `None` if it is unsafe.
fn sanitize(decoded: &str) -> Option<Vec<&str>> {
    let mut segments = Vec::new();
    for segment in decoded.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return None;
        }
        if segment.bytes().any(|b| b < 0x20 || b == b'\\') {
            return None;
        }
        segments.push(segment);
    }
    Some(segments)
}

/// Decodes `%XX` escapes; returns `None` on a malformed escape or non-UTF-8.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_value(bytes[i + 1])?;
            let lo = hex_value(bytes[i + 2])?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// A minimal extension-to-MIME guess. The full table plus config overrides
/// arrive in a later phase.
fn guess_mime(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// An in-memory asset source for tests.
    struct MemAssets {
        files: Vec<(&'static str, &'static [u8])>,
    }

    impl AssetSource for MemAssets {
        fn load(&self, rel_path: &str) -> Option<crate::asset::Asset> {
            self.files
                .iter()
                .find(|(name, _)| *name == rel_path)
                .map(|(_, bytes)| crate::asset::Asset {
                    bytes: bytes.to_vec(),
                })
        }
    }

    fn assets() -> MemAssets {
        MemAssets {
            files: Vec::from([
                ("index.html", b"<h1>home</h1>" as &[u8]),
                ("style.css", b"body{}" as &[u8]),
                ("dir/index.html", b"<h1>dir</h1>" as &[u8]),
            ]),
        }
    }

    fn indexes() -> Vec<String> {
        Vec::from(["index.html".to_string()])
    }

    #[test]
    fn serves_a_known_file_with_mime() {
        let resp = serve_static("/style.css", &indexes(), &assets()).expect("file should serve");
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(resp.body, b"body{}");
        assert!(resp
            .headers
            .iter()
            .any(|(n, v)| n == "Content-Type" && v == "text/css; charset=utf-8"));
    }

    #[test]
    fn root_resolves_index_file() {
        let resp = serve_static("/", &indexes(), &assets()).expect("index should serve");
        assert_eq!(resp.body, b"<h1>home</h1>");
    }

    #[test]
    fn trailing_slash_resolves_subdir_index() {
        let resp = serve_static("/dir/", &indexes(), &assets()).expect("dir index should serve");
        assert_eq!(resp.body, b"<h1>dir</h1>");
    }

    #[test]
    fn missing_file_is_none() {
        assert!(serve_static("/nope.txt", &indexes(), &assets()).is_none());
    }

    #[test]
    fn traversal_is_rejected() {
        // Both raw and percent-encoded ".." must be refused, never served.
        assert!(serve_static("/../secret", &indexes(), &assets()).is_none());
        assert!(serve_static("/%2e%2e/secret", &indexes(), &assets()).is_none());
    }

    #[test]
    fn percent_encoded_path_is_decoded() {
        // "%2F" decodes to '/', then segments split and resolve normally.
        let resp = serve_static("/dir%2Findex.html", &indexes(), &assets())
            .expect("decoded path should serve");
        assert_eq!(resp.body, b"<h1>dir</h1>");
    }

    #[test]
    fn malformed_percent_escape_is_rejected() {
        assert!(serve_static("/style%2", &indexes(), &assets()).is_none());
    }
}
