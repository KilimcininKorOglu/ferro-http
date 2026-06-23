//! Filesystem-backed [`AssetSource`]: serves files from under a root directory.

use std::path::{Component, Path, PathBuf};

use ferro_core::asset::{Asset, AssetSource};

/// Serves static files rooted at `root`.
///
/// The core has already rejected lexical traversal (`..`, control bytes) before
/// calling [`AssetSource::load`]. As defense in depth this also canonicalizes
/// the resolved path and confirms it stays within the canonical root, so a
/// symlink can never escape the document root. When `follow_symlinks` is false
/// it additionally rejects any path that traverses a symlink at all, honoring
/// the stricter no-follow posture even for links that stay inside the root.
pub struct FsAssets {
    root: PathBuf,
    follow_symlinks: bool,
}

impl FsAssets {
    /// Creates an asset source serving from `root`. When `follow_symlinks` is
    /// false, requests whose path crosses a symbolic link are not served.
    pub fn new(root: impl Into<PathBuf>, follow_symlinks: bool) -> FsAssets {
        FsAssets {
            root: root.into(),
            follow_symlinks,
        }
    }

    /// Returns true if any component from `root` down through `rel_path` is a
    /// symbolic link. `rel_path` has already been percent-decoded and cleared of
    /// `..`/absolute components by the core; any non-`Normal` component here is
    /// unexpected and treated as a reason to reject.
    fn traverses_symlink(root: &Path, rel_path: &str) -> bool {
        let mut path = root.to_path_buf();
        for component in Path::new(rel_path).components() {
            match component {
                Component::Normal(part) => {
                    path.push(part);
                    match std::fs::symlink_metadata(&path) {
                        Ok(meta) if meta.file_type().is_symlink() => return true,
                        Ok(_) => {}
                        Err(_) => return true, // cannot stat: reject conservatively
                    }
                }
                Component::CurDir => {}
                _ => return true,
            }
        }
        false
    }
}

impl AssetSource for FsAssets {
    fn load(&self, rel_path: &str) -> Option<Asset> {
        let canonical_root = self.root.canonicalize().ok()?;
        let candidate = canonical_root.join(rel_path).canonicalize().ok()?;
        if !candidate.starts_with(&canonical_root) {
            return None;
        }
        if !self.follow_symlinks && FsAssets::traverses_symlink(&canonical_root, rel_path) {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A throwaway directory unique to this process and tag.
    fn fresh_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("ferro_fsassets_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test root");
        dir
    }

    #[test]
    fn symlink_inside_root_obeys_the_follow_flag() {
        // A symlink that stays inside the root is not an escape; whether it is
        // served must follow the operator's `follow_symlinks` choice, so the
        // config option is not silently ignored.
        let root = fresh_dir("nofollow");
        std::fs::write(root.join("real.txt"), b"real").expect("write real");
        symlink(root.join("real.txt"), root.join("link.txt")).expect("make symlink");

        let no_follow = FsAssets::new(&root, false);
        let follow = FsAssets::new(&root, true);

        // The real file is served regardless of the flag.
        assert!(no_follow.load("real.txt").is_some());
        assert!(follow.load("real.txt").is_some());
        // The symlink is served only when following is enabled.
        assert!(
            no_follow.load("link.txt").is_none(),
            "no-follow must reject an in-root symlink"
        );
        assert!(
            follow.load("link.txt").is_some(),
            "follow must serve an in-root symlink"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn symlink_escaping_root_is_always_rejected() {
        // The canonical-prefix containment must hold even with following on, so
        // a link pointing outside the root never leaks a file.
        let root = fresh_dir("escape");
        let outside = fresh_dir("escape_outside");
        std::fs::write(outside.join("secret.txt"), b"secret").expect("write secret");
        symlink(outside.join("secret.txt"), root.join("leak.txt")).expect("make symlink");

        let follow = FsAssets::new(&root, true);
        assert!(
            follow.load("leak.txt").is_none(),
            "a link out of root must not be served even with follow enabled"
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
