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
