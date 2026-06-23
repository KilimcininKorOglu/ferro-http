//! ferro std-profile binary: loads config, builds the app, runs the event loop.

#![forbid(unsafe_code)]

mod app;
mod fs_assets;
mod fs_config;
#[cfg(feature = "tls")]
mod tls;
mod transport_mio;

use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use ferro_core::config::{Config, ConfigSource};
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::router::{Params, Router};
use ferro_core::security::RateLimiter;

use app::App;
use fs_assets::FsAssets;
use fs_config::FsConfig;
use transport_mio::Options;

/// Liveness endpoint, demonstrating the API router alongside static serving.
fn health(_request: &Request, _params: &Params) -> Response {
    Response::json(StatusCode::OK, "{\"status\":\"ok\"}")
}

fn main() {
    let config = load_config();

    let addr: SocketAddr = match format!("{}:{}", config.server.bind, config.server.port).parse() {
        Ok(addr) => addr,
        Err(_) => {
            eprintln!(
                "[ferro] invalid bind address {}:{}",
                config.server.bind, config.server.port
            );
            std::process::exit(1);
        }
    };

    let mut router = Router::new();
    router.route(Method::Get, "/api/health", health);

    let rate_limiter = config
        .security
        .rate_limit
        .enabled
        .then(|| RateLimiter::new(config.security.rate_limit.clone()));

    let app = App::new(
        router,
        FsAssets::new(&config.static_files.root),
        config.static_files.index_files.clone(),
        config.mime_overrides.clone(),
        rate_limiter,
        config.logging.access_log,
        config.compression.clone(),
    );

    let options = Options {
        idle_timeout: Duration::from_secs(config.server.keep_alive_secs.max(1)),
        max_connections: config.server.max_connections,
        security_headers: config.security.enable_security_headers,
        max_body: config.server.request_max_bytes,
        worker_threads: config.server.worker_threads,
    };

    // Flip the shutdown flag on SIGINT/SIGTERM; reactors observe it and drain.
    let shutdown = Arc::new(AtomicBool::new(false));
    for &signal in &[signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(signal, Arc::clone(&shutdown)) {
            eprintln!("[ferro] could not install signal handler: {e}");
            std::process::exit(1);
        }
    }

    // Build the optional TLS config; an enabled-but-broken cert is fatal.
    #[cfg(feature = "tls")]
    let tls: transport_mio::SharedTls = if config.tls.enabled {
        match tls::server_config(&config.tls.cert_path, &config.tls.key_path) {
            Ok(server_config) => Some(server_config),
            Err(e) => {
                eprintln!("[ferro] tls error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    #[cfg(not(feature = "tls"))]
    let tls: transport_mio::SharedTls = ();

    #[cfg(feature = "tls")]
    let scheme = if config.tls.enabled { "https" } else { "http" };
    #[cfg(not(feature = "tls"))]
    let scheme = "http";

    eprintln!(
        "[ferro] {} listening on {}://{} (root: {})",
        ferro_core::VERSION,
        scheme,
        addr,
        config.static_files.root
    );

    match transport_mio::serve(addr, &app, &options, &tls, &shutdown) {
        Ok(()) => eprintln!("[ferro] graceful shutdown complete"),
        Err(e) => {
            eprintln!("[ferro] fatal: {e}");
            std::process::exit(1);
        }
    }
}

/// Loads configuration: from an explicit path argument, else `config.json` if
/// present, else built-in defaults. A bad explicit path or invalid JSON is
/// fatal; a missing default path silently falls back to defaults.
fn load_config() -> Config {
    let explicit = std::env::args().nth(1);
    let path = explicit
        .clone()
        .unwrap_or_else(|| "config.json".to_string());

    if std::path::Path::new(&path).is_file() {
        match FsConfig::new(&path).load() {
            Ok(config) => {
                eprintln!("[ferro] loaded config from {path}");
                config
            }
            Err(e) => {
                eprintln!("[ferro] config error: {e}");
                std::process::exit(1);
            }
        }
    } else if explicit.is_some() {
        eprintln!("[ferro] config file not found: {path}");
        std::process::exit(1);
    } else {
        eprintln!("[ferro] no config.json found; using defaults");
        Config::default()
    }
}
