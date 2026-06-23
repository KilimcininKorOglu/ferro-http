//! Embedded web admin panel (enabled by the `webui` feature).
//!
//! `WebAdmin` wraps the live [`crate::app::App`]: it authenticates and serves
//! the panel and JSON API under `/admin`, hot-reloads configuration, and
//! delegates everything else to the current App.

mod admin;
mod sha256;
mod stats;

pub use admin::WebAdmin;
