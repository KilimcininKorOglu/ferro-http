//! `ferro-core` — allocation-only `no_std` HTTP core for the ferro web server.
//!
//! This crate holds all protocol logic (parsing, routing, response building,
//! security) and stays free of `std`, sockets, and the filesystem. Concrete
//! transport, asset, config, and clock backends are supplied by the profile
//! binaries (`ferro` for the std profile; an embedded profile for bare-metal).
//!
//! The crate is `no_std` for real builds but links the standard test harness
//! under `cfg(test)`, so host-side unit tests (added from Faz 1 onward) work
//! with the ordinary `cargo test` runner.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

/// Crate version, surfaced so profile binaries can report a build identity.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: confirms the `no_std` core links a host test harness and
    /// exposes its build identity. Real protocol tests arrive in Faz 1.
    #[test]
    fn version_is_exposed() {
        assert!(!VERSION.is_empty());
    }
}
