//! Filesystem-backed [`AssetSource`]: serves files from under a root directory.

use std::path::PathBuf;

use ferro_core::asset::{Asset, AssetSource};

/// Serves static files rooted at `root`.
///
/// The core has already rejected lexical traversal (`..`, control bytes) before
/// calling [`AssetSource::load`]. As defense in depth this also canonicalizes
/// the resolved path and confirms it stays within the canonical root, so a
/// symlink cannot escape the document root.
pub struct FsAssets {
    root: PathBuf,
}

impl FsAssets {
    /// Creates an asset source serving from `root`.
    pub fn new(root: impl Into<PathBuf>) -> FsAssets {
        FsAssets { root: root.into() }
    }
}

impl AssetSource for FsAssets {
    fn load(&self, rel_path: &str) -> Option<Asset> {
        let canonical_root = self.root.canonicalize().ok()?;
        let candidate = canonical_root.join(rel_path).canonicalize().ok()?;
        if !candidate.starts_with(&canonical_root) {
            return None;
        }
        let meta = std::fs::metadata(&candidate).ok()?;
        if !meta.is_file() {
            return None;
        }
        let bytes = std::fs::read(&candidate).ok()?;
        Some(Asset { bytes })
    }
}
