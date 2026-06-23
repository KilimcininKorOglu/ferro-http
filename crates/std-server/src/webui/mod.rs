//! Embedded web admin panel (enabled by the `webui` feature).
//!
//! Built incrementally: the auth/hash primitives land first and are wired into
//! the request path by later increments. Until the admin handler consumes them
//! some items have no non-test caller, so dead_code is allowed at the module
//! root; this allow is removed once the panel is wired in.
#![allow(dead_code)]

mod sha256;
