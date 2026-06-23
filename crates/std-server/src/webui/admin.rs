//! The web admin Service: serves the embedded panel and JSON API under
//! `/admin`, hot-reloads configuration, and delegates every other request to
//! the live [`App`]. Config edits are validated, persisted to disk, and applied
//! by rebuilding the App and swapping it atomically.

use std::path::PathBuf;
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::time::Instant;

use ferro_core::config::Config;
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::service::{RequestContext, Service};

use crate::app::App;
use crate::webui::sha256::{basic_auth_ok, password_matches, sha256_hex};
use crate::webui::stats::Stats;

/// The embedded admin panel, with no external resources.
const PANEL_HTML: &str = include_str!("assets/panel.html");

/// Embedded UI translations, served to the panel as one JSON object.
const I18N_EN: &str = include_str!("assets/i18n/en.json");
const I18N_TR: &str = include_str!("assets/i18n/tr.json");

/// Rebuilds the live [`App`] from a configuration. Supplied by the binary so the
/// admin module stays unaware of route registration.
type AppBuilder = Box<dyn Fn(&Config) -> App + Send + Sync>;

/// Live configuration plus the App built from it; swapped together on reload so
/// they never disagree.
struct AdminState {
    config: Config,
    app: Arc<App>,
}

/// Wraps the live App with the admin panel. `/admin` and `/admin/...` are
/// authenticated and handled here; all other requests are delegated to the
/// current App.
pub struct WebAdmin {
    state: RwLock<AdminState>,
    stats: Stats,
    config_path: PathBuf,
    build_app: AppBuilder,
    start: Instant,
}

impl WebAdmin {
    /// Builds the admin wrapper around an initial config. `build_app` is used
    /// both now and on every hot-reload.
    pub fn new(config: Config, config_path: PathBuf, build_app: AppBuilder) -> WebAdmin {
        let app = Arc::new(build_app(&config));
        WebAdmin {
            state: RwLock::new(AdminState { config, app }),
            stats: Stats::new(),
            config_path,
            build_app,
            start: Instant::now(),
        }
    }

    fn read_state(&self) -> RwLockReadGuard<'_, AdminState> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Routes an already-authenticated admin request.
    fn handle_admin(&self, request: &Request) -> Response {
        match (&request.method, request.path()) {
            (Method::Get, "/admin") | (Method::Get, "/admin/") => panel_response(),
            (Method::Get, "/admin/api/config") => {
                Response::json(StatusCode::OK, &self.read_state().config.to_json_string())
            }
            (Method::Put, "/admin/api/config") => self.put_config(&request.body),
            (Method::Get, "/admin/api/stats") => {
                let uptime = self.start.elapsed().as_secs();
                Response::json(StatusCode::OK, &self.stats.snapshot_json(uptime))
            }
            (Method::Get, "/admin/api/i18n") => Response::json(
                StatusCode::OK,
                &format!("{{\"en\":{},\"tr\":{}}}", I18N_EN.trim(), I18N_TR.trim()),
            ),
            (Method::Post, "/admin/api/password") => self.change_password(&request.body),
            _ => json_error(StatusCode::NOT_FOUND, "no such admin endpoint"),
        }
    }

    /// Validates, persists, and hot-applies a new configuration.
    fn put_config(&self, body: &[u8]) -> Response {
        let text = match core::str::from_utf8(body) {
            Ok(text) => text,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "body is not valid UTF-8"),
        };
        let new_config = match Config::from_json_str(text) {
            Ok(config) => config,
            Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
        };
        // Persist to disk before swapping, so a write failure leaves the running
        // config untouched.
        if let Err(e) = std::fs::write(&self.config_path, new_config.to_json_string()) {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not write config: {e}"),
            );
        }
        // Fields that the running reactors cannot change without a restart.
        let restart = {
            let current = self.read_state();
            restart_required_fields(&current.config, &new_config)
        };
        let app = Arc::new((self.build_app)(&new_config));
        {
            let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
            state.config = new_config;
            state.app = app;
        }
        let list = restart
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(",");
        Response::json(
            StatusCode::OK,
            &format!("{{\"ok\":true,\"restart_required\":[{list}]}}"),
        )
    }

    /// Changes the admin password: verifies the current password, hashes the
    /// new one, persists the config to disk, and updates the live credentials.
    /// Body: `{"current_password": "...", "new_password": "..."}`.
    fn change_password(&self, body: &[u8]) -> Response {
        let text = match core::str::from_utf8(body) {
            Ok(text) => text,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "body is not valid UTF-8"),
        };
        let parsed = match ferro_core::json::parse(text) {
            Ok(value) => value,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid JSON body"),
        };
        let current = parsed
            .get("current_password")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_password = parsed
            .get("new_password")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if new_password.is_empty() {
            return json_error(StatusCode::BAD_REQUEST, "new password must not be empty");
        }
        // Verify the current password against the running hash, then build the
        // updated config off it.
        let new_config = {
            let state = self.read_state();
            if !password_matches(current, &state.config.admin.password_sha256) {
                return json_error(StatusCode::UNAUTHORIZED, "current password is incorrect");
            }
            let mut config = state.config.clone();
            config.admin.password_sha256 = sha256_hex(new_password.as_bytes());
            config
        };
        // Persist before swapping, so a write failure leaves credentials intact.
        if let Err(e) = std::fs::write(&self.config_path, new_config.to_json_string()) {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not write config: {e}"),
            );
        }
        {
            let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
            state.config = new_config;
        }
        Response::json(StatusCode::OK, "{\"ok\":true}")
    }

    /// True when the request carries valid admin credentials.
    fn authenticated(&self, request: &Request) -> bool {
        let state = self.read_state();
        if state.config.admin.username.is_empty() {
            return false;
        }
        request
            .header("authorization")
            .map(|h| {
                basic_auth_ok(
                    h,
                    &state.config.admin.username,
                    &state.config.admin.password_sha256,
                )
            })
            .unwrap_or(false)
    }
}

impl Service for WebAdmin {
    fn handle(&self, request: &Request, ctx: &RequestContext) -> Response {
        let path = request.path();
        let is_admin = path == "/admin" || path.starts_with("/admin/");
        let response = if is_admin {
            if self.authenticated(request) {
                self.handle_admin(request)
            } else {
                unauthorized_response()
            }
        } else {
            // Delegate to the live App; clone the Arc so the lock is released
            // before handling (which may do file I/O).
            let app = self.read_state().app.clone();
            app.handle(request, ctx)
        };
        self.stats
            .record(response.status.code(), response.body.len());
        response
    }
}

/// Server settings that only take effect on restart (sockets/threads/TLS).
fn restart_required_fields(old: &Config, new: &Config) -> Vec<String> {
    let mut fields = Vec::new();
    if old.server.bind != new.server.bind {
        fields.push("server.bind".to_string());
    }
    if old.server.port != new.server.port {
        fields.push("server.port".to_string());
    }
    if old.server.worker_threads != new.server.worker_threads {
        fields.push("server.worker_threads".to_string());
    }
    if old.tls != new.tls {
        fields.push("tls".to_string());
    }
    fields
}

fn panel_response() -> Response {
    Response {
        status: StatusCode::OK,
        headers: vec![(
            "Content-Type".to_string(),
            "text/html; charset=utf-8".to_string(),
        )],
        body: PANEL_HTML
            .replace("{{VERSION}}", ferro_core::VERSION)
            .into_bytes(),
    }
}

fn unauthorized_response() -> Response {
    Response {
        status: StatusCode::UNAUTHORIZED,
        headers: vec![
            (
                "WWW-Authenticate".to_string(),
                "Basic realm=\"ferro admin\"".to_string(),
            ),
            (
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            ),
        ],
        body: b"Unauthorized\n".to_vec(),
    }
}

fn json_error(status: StatusCode, message: &str) -> Response {
    Response::json(
        status,
        &format!("{{\"error\":\"{}\"}}", json_escape(message)),
    )
}

/// Minimal JSON string escaping for short error messages.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}
