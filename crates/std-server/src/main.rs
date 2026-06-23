//! ferro std-profile binary: loads config, builds the app, runs the event loop.

#![forbid(unsafe_code)]

mod app;
mod fs_assets;
mod fs_config;
mod transport_mio;

use std::net::SocketAddr;
use std::time::Duration;

use ferro_core::config::{Config, ConfigSource};
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::router::{Params, Router};

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

    let app = App::new(
        router,
        FsAssets::new(&config.static_files.root),
        config.static_files.index_files.clone(),
        config.mime_overrides.clone(),
    );

    let options = Options {
        idle_timeout: Duration::from_secs(config.server.keep_alive_secs.max(1)),
        max_connections: config.server.max_connections,
    };

    eprintln!(
        "[ferro] {} listening on http://{} (root: {})",
        ferro_core::VERSION,
        addr,
        config.static_files.root
    );

    if let Err(e) = transport_mio::serve(addr, &app, &options) {
        eprintln!("[ferro] fatal: {e}");
        std::process::exit(1);
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
