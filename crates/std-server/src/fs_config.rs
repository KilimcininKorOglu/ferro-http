//! Filesystem-backed [`ConfigSource`]: reads and parses `config.json`.

use std::path::PathBuf;

use ferro_core::config::{Config, ConfigError, ConfigSource};

/// Loads configuration from a JSON file on disk.
pub struct FsConfig {
    path: PathBuf,
}

impl FsConfig {
    /// Creates a config source reading from `path`.
    pub fn new(path: impl Into<PathBuf>) -> FsConfig {
        FsConfig { path: path.into() }
    }
}

impl ConfigSource for FsConfig {
    fn load(&self) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| ConfigError::Source(format!("{}: {}", self.path.display(), e)))?;
        Config::from_json_str(&text)
    }
}
