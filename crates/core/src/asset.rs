//! Abstraction over where static assets come from.

use alloc::vec::Vec;

/// A loaded static asset.
///
/// Today this owns the asset bytes. In the std profile that means the file was
/// read into memory; the embedded profile points at compile-time bytes. A later
/// phase turns this into a true handle (a file descriptor) so the std transport
/// can serve it with zero-copy `sendfile`, without changing this trait.
pub struct Asset {
    pub bytes: Vec<u8>,
    /// Last-modified time in Unix seconds, when the source can provide one. It
    /// drives `Last-Modified`/`ETag` and conditional-request handling. Sources
    /// without a clock (such as compile-time embedded assets) use `None`, which
    /// serves the asset without validators.
    pub mtime: Option<u64>,
}

/// Supplies static asset bytes for a validated relative path.
///
/// Implementations receive a path that the core has already sanitized against
/// traversal; they must still refuse anything that escapes their own root.
pub trait AssetSource {
    /// Loads the asset at `rel_path` (relative, no leading slash), or `None`
    /// when it does not exist or is not a regular file.
    fn load(&self, rel_path: &str) -> Option<Asset>;
}

/// An [`AssetSource`] backed by a compile-time table of `(path, bytes)` pairs.
///
/// This is the embedded profile's asset source: the table is built with
/// [`include_bytes!`](core::include_bytes) so the document root ships inside the
/// firmware image, with no filesystem at runtime. Keys are the same relative
/// paths the core resolves to (for example `"index.html"`, `"dir/index.html"`).
///
/// Because the bytes are `'static`, no traversal is possible; [`load`] only ever
/// returns a copy of an entry the program itself compiled in.
///
/// [`load`]: AssetSource::load
///
/// # Examples
///
/// ```
/// use ferro_core::asset::{AssetSource, EmbeddedAssets};
///
/// static FILES: &[(&str, &[u8])] = &[("index.html", b"<h1>hi</h1>")];
/// let assets = EmbeddedAssets::new(FILES);
/// assert_eq!(assets.load("index.html").unwrap().bytes, b"<h1>hi</h1>");
/// assert!(assets.load("missing").is_none());
/// ```
pub struct EmbeddedAssets {
    table: &'static [(&'static str, &'static [u8])],
}

impl EmbeddedAssets {
    /// Creates an asset source over a compile-time `(path, bytes)` table.
    pub const fn new(table: &'static [(&'static str, &'static [u8])]) -> EmbeddedAssets {
        EmbeddedAssets { table }
    }
}

impl AssetSource for EmbeddedAssets {
    fn load(&self, rel_path: &str) -> Option<Asset> {
        self.table
            .iter()
            .find(|(name, _)| *name == rel_path)
            .map(|(_, bytes)| Asset {
                bytes: bytes.to_vec(),
                // Compile-time assets have no filesystem clock, so they serve
                // without a Last-Modified/ETag validator.
                mtime: None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FILES: &[(&str, &[u8])] = &[
        ("index.html", b"<h1>home</h1>"),
        ("dir/index.html", b"<h1>dir</h1>"),
    ];

    #[test]
    fn loads_a_table_entry() {
        let assets = EmbeddedAssets::new(FILES);
        assert_eq!(assets.load("index.html").unwrap().bytes, b"<h1>home</h1>");
        assert_eq!(
            assets.load("dir/index.html").unwrap().bytes,
            b"<h1>dir</h1>"
        );
    }

    #[test]
    fn unknown_path_is_none() {
        let assets = EmbeddedAssets::new(FILES);
        assert!(assets.load("nope.txt").is_none());
        // Lookups are exact: a leading slash never matches a table key.
        assert!(assets.load("/index.html").is_none());
    }
}
