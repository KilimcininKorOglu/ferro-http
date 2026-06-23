//! Extension-to-MIME resolution with configuration overrides.

use alloc::string::String;

/// Resolves the MIME type for a file name. Config `overrides` (keyed by dotted
/// extension, e.g. `.wasm`) win over the built-in table; an unknown extension
/// falls back to `application/octet-stream`.
pub fn mime_for<'a>(name: &str, overrides: &'a [(String, String)]) -> &'a str {
    let dotted = match name.rfind('.') {
        Some(i) => &name[i..],
        None => "",
    };
    if !dotted.is_empty() {
        for (ext, ty) in overrides {
            if ext.eq_ignore_ascii_case(dotted) {
                return ty.as_str();
            }
        }
    }
    builtin_mime(dotted.strip_prefix('.').unwrap_or(""))
}

fn builtin_mime(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "wasm" => "application/wasm",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec::Vec;

    fn no_overrides() -> Vec<(String, String)> {
        Vec::new()
    }

    #[test]
    fn resolves_common_extensions() {
        assert_eq!(
            mime_for("index.html", &no_overrides()),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            mime_for("app.js", &no_overrides()),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_for("photo.JPG", &no_overrides()), "image/jpeg");
    }

    #[test]
    fn unknown_extension_falls_back() {
        assert_eq!(
            mime_for("data.bin", &no_overrides()),
            "application/octet-stream"
        );
        assert_eq!(
            mime_for("noext", &no_overrides()),
            "application/octet-stream"
        );
    }

    #[test]
    fn override_wins_over_builtin() {
        // A config override replaces the built-in mapping for that extension.
        let overrides = Vec::from([(".json".to_string(), "application/x-custom".to_string())]);
        assert_eq!(mime_for("data.json", &overrides), "application/x-custom");
        // Other extensions are unaffected by the override.
        assert_eq!(mime_for("a.css", &overrides), "text/css; charset=utf-8");
    }
}
