//! Server configuration: schema, defaults, and JSON loading.
//!
//! [`Config`] mirrors the documented `config.json` schema. Every field has a
//! default, so a partial config is valid and missing fields fall back rather
//! than erroring. Malformed JSON, wrong types, and out-of-range values fail
//! loudly with a path-qualified [`ConfigError`].
//!
//! [`ConfigSource`] abstracts where the JSON comes from: the std profile reads
//! a file from disk, the embedded profile embeds it at compile time. The core
//! only knows how to parse and validate.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::json::{self, JsonError, JsonValue};

/// Network and connection settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    /// Worker count; `0` means "derive from available parallelism".
    pub worker_threads: usize,
    pub max_connections: usize,
    pub keep_alive_secs: u64,
    pub read_timeout_secs: u64,
    pub request_max_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            bind: "0.0.0.0".to_string(),
            port: 8080,
            worker_threads: 0,
            max_connections: 1024,
            keep_alive_secs: 15,
            read_timeout_secs: 30,
            request_max_bytes: 1024 * 1024,
        }
    }
}

/// Static file serving settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticConfig {
    pub root: String,
    pub index_files: Vec<String>,
    pub follow_symlinks: bool,
    pub directory_listing: bool,
}

impl Default for StaticConfig {
    fn default() -> Self {
        StaticConfig {
            root: "./public".to_string(),
            index_files: Vec::from(["index.html".to_string(), "index.htm".to_string()]),
            follow_symlinks: false,
            directory_listing: false,
        }
    }
}

/// Response compression settings (effective only when the `gzip` feature is on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressionConfig {
    pub gzip: bool,
    pub min_bytes: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        CompressionConfig {
            gzip: true,
            min_bytes: 1024,
        }
    }
}

/// Per-IP rate limiting settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub requests: u64,
    pub window_secs: u64,
    pub ban_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        RateLimitConfig {
            enabled: true,
            requests: 600,
            window_secs: 600,
            ban_secs: 300,
        }
    }
}

/// Security settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecurityConfig {
    pub enable_security_headers: bool,
    pub rate_limit: RateLimitConfig,
    pub blocked_patterns: Vec<String>,
}

/// Logging settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingConfig {
    pub level: String,
    pub access_log: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: "info".to_string(),
            access_log: true,
        }
    }
}

/// TLS settings. Effective only in the std profile built with the `tls`
/// feature; the core merely parses and validates them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TlsConfig {
    pub enabled: bool,
    pub cert_path: String,
    pub key_path: String,
}

/// The complete server configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub server: ServerConfig,
    pub static_files: StaticConfig,
    pub compression: CompressionConfig,
    pub security: SecurityConfig,
    pub tls: TlsConfig,
    pub logging: LoggingConfig,
    /// File-extension to MIME-type overrides, e.g. `.wasm` -> `application/wasm`.
    pub mime_overrides: Vec<(String, String)>,
}

const LOG_LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

impl Config {
    /// Parses and validates a configuration from a JSON string. Missing fields
    /// keep their defaults.
    pub fn from_json_str(text: &str) -> Result<Config, ConfigError> {
        let root = json::parse(text).map_err(ConfigError::Json)?;
        Config::from_json(&root)
    }

    /// Builds a configuration from an already-parsed JSON value.
    pub fn from_json(root: &JsonValue) -> Result<Config, ConfigError> {
        if !matches!(root, JsonValue::Object(_)) {
            return Err(ConfigError::NotAnObject);
        }
        let mut cfg = Config::default();

        if let Some(s) = root.get("server") {
            read_server(s, &mut cfg.server)?;
        }
        if let Some(s) = root.get("static") {
            read_static(s, &mut cfg.static_files)?;
        }
        if let Some(c) = root.get("compression") {
            read_bool(c, "gzip", &mut cfg.compression.gzip, "compression")?;
            read_usize(
                c,
                "min_bytes",
                &mut cfg.compression.min_bytes,
                "compression",
            )?;
        }
        if let Some(s) = root.get("security") {
            read_security(s, &mut cfg.security)?;
        }
        if let Some(t) = root.get("tls") {
            read_tls(t, &mut cfg.tls)?;
        }
        if let Some(l) = root.get("logging") {
            read_string(l, "level", &mut cfg.logging.level, "logging")?;
            read_bool(l, "access_log", &mut cfg.logging.access_log, "logging")?;
        }
        if let Some(m) = root.get("mime_overrides") {
            cfg.mime_overrides = read_string_map(m, "mime_overrides")?;
        }

        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.server.port == 0 {
            return Err(ConfigError::invalid("server.port", "must be 1-65535"));
        }
        if self.server.bind.is_empty() {
            return Err(ConfigError::invalid("server.bind", "must not be empty"));
        }
        if self.static_files.root.is_empty() {
            return Err(ConfigError::invalid("static.root", "must not be empty"));
        }
        if self.tls.enabled {
            if self.tls.cert_path.is_empty() {
                return Err(ConfigError::invalid(
                    "tls.cert_path",
                    "required when tls.enabled is true",
                ));
            }
            if self.tls.key_path.is_empty() {
                return Err(ConfigError::invalid(
                    "tls.key_path",
                    "required when tls.enabled is true",
                ));
            }
        }
        if !LOG_LEVELS.contains(&self.logging.level.as_str()) {
            return Err(ConfigError::invalid(
                "logging.level",
                "must be error|warn|info|debug|trace",
            ));
        }
        Ok(())
    }
}

/// Abstracts the origin of configuration bytes (filesystem, embedded, ...).
pub trait ConfigSource {
    /// Loads, parses, and validates the configuration.
    fn load(&self) -> Result<Config, ConfigError>;
}

/// Why a configuration could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Json(JsonError),
    NotAnObject,
    WrongType {
        path: String,
    },
    InvalidValue {
        path: String,
        reason: &'static str,
    },
    /// The config source (e.g. a file) failed to provide bytes.
    Source(String),
}

impl ConfigError {
    fn wrong_type(path: &str) -> ConfigError {
        ConfigError::WrongType {
            path: path.to_string(),
        }
    }

    fn invalid(path: &str, reason: &'static str) -> ConfigError {
        ConfigError::InvalidValue {
            path: path.to_string(),
            reason,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Json(e) => write!(f, "invalid JSON: {e}"),
            ConfigError::NotAnObject => f.write_str("config root must be a JSON object"),
            ConfigError::WrongType { path } => write!(f, "wrong type for {path}"),
            ConfigError::InvalidValue { path, reason } => write!(f, "invalid {path}: {reason}"),
            ConfigError::Source(msg) => write!(f, "config source error: {msg}"),
        }
    }
}

fn read_server(value: &JsonValue, server: &mut ServerConfig) -> Result<(), ConfigError> {
    read_string(value, "bind", &mut server.bind, "server")?;
    read_u16(value, "port", &mut server.port, "server")?;
    read_usize(
        value,
        "worker_threads",
        &mut server.worker_threads,
        "server",
    )?;
    read_usize(
        value,
        "max_connections",
        &mut server.max_connections,
        "server",
    )?;
    read_u64(
        value,
        "keep_alive_secs",
        &mut server.keep_alive_secs,
        "server",
    )?;
    read_u64(
        value,
        "read_timeout_secs",
        &mut server.read_timeout_secs,
        "server",
    )?;
    read_usize(
        value,
        "request_max_bytes",
        &mut server.request_max_bytes,
        "server",
    )?;
    Ok(())
}

fn read_static(value: &JsonValue, cfg: &mut StaticConfig) -> Result<(), ConfigError> {
    read_string(value, "root", &mut cfg.root, "static")?;
    read_bool(value, "follow_symlinks", &mut cfg.follow_symlinks, "static")?;
    read_bool(
        value,
        "directory_listing",
        &mut cfg.directory_listing,
        "static",
    )?;
    if let Some(v) = value.get("index_files") {
        cfg.index_files = read_string_array(v, "static.index_files")?;
    }
    Ok(())
}

fn read_tls(value: &JsonValue, tls: &mut TlsConfig) -> Result<(), ConfigError> {
    read_bool(value, "enabled", &mut tls.enabled, "tls")?;
    read_string(value, "cert_path", &mut tls.cert_path, "tls")?;
    read_string(value, "key_path", &mut tls.key_path, "tls")?;
    Ok(())
}

fn read_security(value: &JsonValue, cfg: &mut SecurityConfig) -> Result<(), ConfigError> {
    read_bool(
        value,
        "enable_security_headers",
        &mut cfg.enable_security_headers,
        "security",
    )?;
    if let Some(rl) = value.get("rate_limit") {
        read_bool(
            rl,
            "enabled",
            &mut cfg.rate_limit.enabled,
            "security.rate_limit",
        )?;
        read_u64(
            rl,
            "requests",
            &mut cfg.rate_limit.requests,
            "security.rate_limit",
        )?;
        read_u64(
            rl,
            "window_secs",
            &mut cfg.rate_limit.window_secs,
            "security.rate_limit",
        )?;
        read_u64(
            rl,
            "ban_secs",
            &mut cfg.rate_limit.ban_secs,
            "security.rate_limit",
        )?;
    }
    if let Some(bp) = value.get("blocked_patterns") {
        cfg.blocked_patterns = read_string_array(bp, "security.blocked_patterns")?;
    }
    Ok(())
}

fn read_bool(obj: &JsonValue, key: &str, slot: &mut bool, path: &str) -> Result<(), ConfigError> {
    if let Some(v) = obj.get(key) {
        *slot = v
            .as_bool()
            .ok_or_else(|| ConfigError::wrong_type(&format!("{path}.{key}")))?;
    }
    Ok(())
}

fn read_string(
    obj: &JsonValue,
    key: &str,
    slot: &mut String,
    path: &str,
) -> Result<(), ConfigError> {
    if let Some(v) = obj.get(key) {
        let s = v
            .as_str()
            .ok_or_else(|| ConfigError::wrong_type(&format!("{path}.{key}")))?;
        *slot = s.to_string();
    }
    Ok(())
}

fn read_u64(obj: &JsonValue, key: &str, slot: &mut u64, path: &str) -> Result<(), ConfigError> {
    if let Some(v) = obj.get(key) {
        *slot = v
            .as_u64()
            .ok_or_else(|| ConfigError::wrong_type(&format!("{path}.{key}")))?;
    }
    Ok(())
}

fn read_usize(obj: &JsonValue, key: &str, slot: &mut usize, path: &str) -> Result<(), ConfigError> {
    if let Some(v) = obj.get(key) {
        let n = v
            .as_u64()
            .ok_or_else(|| ConfigError::wrong_type(&format!("{path}.{key}")))?;
        if n > usize::MAX as u64 {
            return Err(ConfigError::invalid_owned(path, key, "value too large"));
        }
        *slot = n as usize;
    }
    Ok(())
}

fn read_u16(obj: &JsonValue, key: &str, slot: &mut u16, path: &str) -> Result<(), ConfigError> {
    if let Some(v) = obj.get(key) {
        let n = v
            .as_u64()
            .ok_or_else(|| ConfigError::wrong_type(&format!("{path}.{key}")))?;
        if n > u16::MAX as u64 {
            return Err(ConfigError::invalid_owned(path, key, "must be 1-65535"));
        }
        *slot = n as u16;
    }
    Ok(())
}

fn read_string_array(value: &JsonValue, path: &str) -> Result<Vec<String>, ConfigError> {
    let items = value
        .as_array()
        .ok_or_else(|| ConfigError::wrong_type(path))?;
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let s = item
            .as_str()
            .ok_or_else(|| ConfigError::wrong_type(&format!("{path}[{i}]")))?;
        out.push(s.to_string());
    }
    Ok(out)
}

fn read_string_map(value: &JsonValue, path: &str) -> Result<Vec<(String, String)>, ConfigError> {
    match value {
        JsonValue::Object(members) => {
            let mut out = Vec::new();
            for (k, v) in members {
                let s = v
                    .as_str()
                    .ok_or_else(|| ConfigError::wrong_type(&format!("{path}.{k}")))?;
                out.push((k.clone(), s.to_string()));
            }
            Ok(out)
        }
        _ => Err(ConfigError::wrong_type(path)),
    }
}

impl ConfigError {
    fn invalid_owned(path: &str, key: &str, reason: &'static str) -> ConfigError {
        ConfigError::InvalidValue {
            path: format!("{path}.{key}"),
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_object_yields_defaults() {
        let cfg = Config::from_json_str("{}").unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.static_files.root, "./public");
    }

    #[test]
    fn partial_config_overrides_only_named_fields() {
        // Only the port is set; everything else must remain at its default.
        let cfg = Config::from_json_str(r#"{"server": {"port": 9000}}"#).unwrap();
        assert_eq!(cfg.server.port, 9000);
        assert_eq!(cfg.server.bind, "0.0.0.0");
        assert_eq!(cfg.server.max_connections, 1024);
    }

    #[test]
    fn full_config_parses() {
        let text = r#"{
            "server": {"bind": "127.0.0.1", "port": 3000, "worker_threads": 4,
                       "max_connections": 2048, "keep_alive_secs": 5,
                       "read_timeout_secs": 10, "request_max_bytes": 2048},
            "static": {"root": "./www", "index_files": ["home.html"],
                       "follow_symlinks": true, "directory_listing": true},
            "compression": {"gzip": false, "min_bytes": 512},
            "security": {"enable_security_headers": true,
                         "rate_limit": {"enabled": false, "requests": 10,
                                        "window_secs": 60, "ban_secs": 120},
                         "blocked_patterns": ["/admin"]},
            "mime_overrides": {".wasm": "application/wasm"},
            "logging": {"level": "debug", "access_log": false}
        }"#;
        let cfg = Config::from_json_str(text).unwrap();
        assert_eq!(cfg.server.bind, "127.0.0.1");
        assert_eq!(cfg.server.port, 3000);
        assert_eq!(cfg.static_files.index_files, ["home.html"]);
        assert!(!cfg.compression.gzip);
        assert!(!cfg.security.rate_limit.enabled);
        assert_eq!(cfg.security.blocked_patterns, ["/admin"]);
        assert_eq!(
            cfg.mime_overrides,
            [(".wasm".to_string(), "application/wasm".to_string())]
        );
        assert_eq!(cfg.logging.level, "debug");
    }

    #[test]
    fn wrong_type_is_reported_with_path() {
        let err = Config::from_json_str(r#"{"server": {"port": "nope"}}"#).unwrap_err();
        assert_eq!(
            err,
            ConfigError::WrongType {
                path: "server.port".to_string()
            }
        );
    }

    #[test]
    fn port_out_of_range_is_rejected() {
        let err = Config::from_json_str(r#"{"server": {"port": 70000}}"#).unwrap_err();
        assert_eq!(
            err,
            ConfigError::InvalidValue {
                path: "server.port".to_string(),
                reason: "must be 1-65535"
            }
        );
    }

    #[test]
    fn zero_port_fails_validation() {
        let err = Config::from_json_str(r#"{"server": {"port": 0}}"#).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn unknown_log_level_is_rejected() {
        let err = Config::from_json_str(r#"{"logging": {"level": "verbose"}}"#).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn malformed_json_surfaces_as_json_error() {
        let err = Config::from_json_str("{").unwrap_err();
        assert!(matches!(err, ConfigError::Json(_)));
    }

    #[test]
    fn non_object_root_is_rejected() {
        assert_eq!(
            Config::from_json_str("[]").unwrap_err(),
            ConfigError::NotAnObject
        );
    }

    #[test]
    fn tls_is_disabled_by_default() {
        let cfg = Config::default();
        assert!(!cfg.tls.enabled);
        assert!(cfg.tls.cert_path.is_empty());
    }

    #[test]
    fn tls_section_parses() {
        let cfg = Config::from_json_str(
            r#"{"tls": {"enabled": true, "cert_path": "/c.pem", "key_path": "/k.pem"}}"#,
        )
        .expect("valid tls config");
        assert!(cfg.tls.enabled);
        assert_eq!(cfg.tls.cert_path, "/c.pem");
        assert_eq!(cfg.tls.key_path, "/k.pem");
    }

    #[test]
    fn tls_enabled_without_cert_is_rejected() {
        // Enabling TLS without the cert/key paths must fail loudly, not serve plaintext.
        let err = Config::from_json_str(r#"{"tls": {"enabled": true}}"#).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidValue { path, .. } if path == "tls.cert_path"
        ));
    }
}
