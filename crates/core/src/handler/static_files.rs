//! Static file serving over an [`AssetSource`], with lexical path safety.
//!
//! Path safety here is purely lexical (percent-decode, then reject `..`,
//! backslashes, and control bytes). Canonical-prefix and symlink hardening,
//! which need the filesystem, live in the std `AssetSource` and a later phase.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::asset::{Asset, AssetSource};
use crate::http::date::http_date;
use crate::http::mime::mime_for;
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
    mime_overrides: &[(String, String)],
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
                return Some(file_response(&candidate, asset, mime_overrides));
            }
        }
        None
    } else {
        let rel = segments.join("/");
        assets
            .load(&rel)
            .map(|asset| file_response(&rel, asset, mime_overrides))
    }
}

/// Builds the `200 OK` response for an asset. `Accept-Ranges` is always
/// advertised; `Last-Modified` and a strong `ETag` are added when the source
/// provides an mtime, so the caller can short-circuit conditional requests.
fn file_response(name: &str, asset: Asset, mime_overrides: &[(String, String)]) -> Response {
    let mut response = Response::new(StatusCode::OK)
        .with_header("Content-Type", mime_for(name, mime_overrides))
        .with_header("Accept-Ranges", "bytes");
    if let Some(mtime) = asset.mtime {
        response = response
            .with_header("Last-Modified", &http_date(mtime))
            .with_header("ETag", &etag(asset.bytes.len(), mtime));
    }
    response.body = asset.bytes;
    response
}

/// A strong validator derived from the asset length and mtime: `"<len>-<mtime>"`.
fn etag(len: usize, mtime: u64) -> String {
    format!("\"{len:x}-{mtime:x}\"")
}

/// Whether `path` resolves to an existing static asset, using the same
/// resolution as [`serve_static`]. Lets a caller answer OPTIONS/405 for a static
/// resource (RFC 9110 15.5.6 / 9.3.7) instead of a misleading 404.
pub fn static_exists<A: AssetSource>(path: &str, index_files: &[String], assets: &A) -> bool {
    serve_static(path, index_files, assets, &[]).is_some()
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// An in-memory asset source for tests.
    struct MemAssets {
        files: Vec<(&'static str, &'static [u8])>,
    }

    /// A fixed mtime so validator and conditional assertions are deterministic.
    const MEM_MTIME: u64 = 1_582_934_400;

    impl AssetSource for MemAssets {
        fn load(&self, rel_path: &str) -> Option<crate::asset::Asset> {
            self.files
                .iter()
                .find(|(name, _)| *name == rel_path)
                .map(|(_, bytes)| crate::asset::Asset {
                    bytes: bytes.to_vec(),
                    mtime: Some(MEM_MTIME),
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

    fn no_mime() -> Vec<(String, String)> {
        Vec::new()
    }

    #[test]
    fn serves_a_known_file_with_mime() {
        let resp = serve_static("/style.css", &indexes(), &assets(), &no_mime())
            .expect("file should serve");
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(resp.body, b"body{}");
        assert!(resp
            .headers
            .iter()
            .any(|(n, v)| n == "Content-Type" && v == "text/css; charset=utf-8"));
    }

    #[test]
    fn root_resolves_index_file() {
        let resp =
            serve_static("/", &indexes(), &assets(), &no_mime()).expect("index should serve");
        assert_eq!(resp.body, b"<h1>home</h1>");
    }

    #[test]
    fn trailing_slash_resolves_subdir_index() {
        let resp = serve_static("/dir/", &indexes(), &assets(), &no_mime())
            .expect("dir index should serve");
        assert_eq!(resp.body, b"<h1>dir</h1>");
    }

    #[test]
    fn missing_file_is_none() {
        assert!(serve_static("/nope.txt", &indexes(), &assets(), &no_mime()).is_none());
    }

    #[test]
    fn traversal_is_rejected() {
        // Both raw and percent-encoded ".." must be refused, never served.
        assert!(serve_static("/../secret", &indexes(), &assets(), &no_mime()).is_none());
        assert!(serve_static("/%2e%2e/secret", &indexes(), &assets(), &no_mime()).is_none());
    }

    #[test]
    fn percent_encoded_path_is_decoded() {
        // "%2F" decodes to '/', then segments split and resolve normally.
        let resp = serve_static("/dir%2Findex.html", &indexes(), &assets(), &no_mime())
            .expect("decoded path should serve");
        assert_eq!(resp.body, b"<h1>dir</h1>");
    }

    #[test]
    fn malformed_percent_escape_is_rejected() {
        assert!(serve_static("/style%2", &indexes(), &assets(), &no_mime()).is_none());
    }

    #[test]
    fn served_file_advertises_validators_and_ranges() {
        // A file from a source with an mtime must carry Last-Modified, a strong
        // ETag, and Accept-Ranges so clients can revalidate and range-request.
        let resp = serve_static("/style.css", &indexes(), &assets(), &no_mime()).expect("serve");
        let has = |n: &str, v: &str| resp.headers.iter().any(|(hn, hv)| hn == n && hv == v);
        assert!(has("Accept-Ranges", "bytes"));
        assert!(has("Last-Modified", &http_date(MEM_MTIME)));
        // "body{}" is 6 bytes -> ETag "6-<hex mtime>".
        assert!(has("ETag", &format!("\"6-{MEM_MTIME:x}\"")));
    }
}
