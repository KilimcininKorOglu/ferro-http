//! TLS termination via rustls (enabled by the `tls` feature).
//!
//! Loads a PEM certificate chain and private key and builds a shared
//! [`rustls::ServerConfig`]; the transport wraps each accepted socket in a
//! per-connection `rustls::ServerConnection` built from it. The crypto backend
//! is `ring`, which builds with a C compiler and needs no cmake.

use std::fmt;
use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;

/// Why TLS could not be initialized.
#[derive(Debug)]
pub enum TlsError {
    /// A certificate or key file could not be read.
    Io(io::Error),
    /// The key file contained no private key.
    NoPrivateKey,
    /// rustls rejected the certificate, key, or configuration.
    Rustls(rustls::Error),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Io(e) => write!(f, "tls io error: {e}"),
            TlsError::NoPrivateKey => f.write_str("no private key found in key file"),
            TlsError::Rustls(e) => write!(f, "rustls error: {e}"),
        }
    }
}

impl std::error::Error for TlsError {}

impl From<io::Error> for TlsError {
    fn from(err: io::Error) -> TlsError {
        TlsError::Io(err)
    }
}

impl From<rustls::Error> for TlsError {
    fn from(err: rustls::Error) -> TlsError {
        TlsError::Rustls(err)
    }
}

/// Loads the cert chain and key from PEM files and builds a shared server config.
///
/// The result is wrapped in an `Arc` so one config is shared across reactor
/// threads, each building a per-connection `ServerConnection` from it.
pub fn server_config(cert_path: &str, key_path: &str) -> Result<Arc<ServerConfig>, TlsError> {
    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
    Ok(Arc::new(config))
}

/// Reads a PEM certificate chain.
fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let mut reader = BufReader::new(File::open(path)?);
    let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
    Ok(certs)
}

/// Reads the first PEM private key (PKCS#8, PKCS#1, or SEC1).
fn load_key(path: &str) -> Result<PrivateKeyDer<'static>, TlsError> {
    let mut reader = BufReader::new(File::open(path)?);
    rustls_pemfile::private_key(&mut reader)?.ok_or(TlsError::NoPrivateKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_cert_file_is_io_error() {
        let err = server_config("/no/such/cert.pem", "/no/such/key.pem").unwrap_err();
        assert!(matches!(err, TlsError::Io(_)));
    }

    #[test]
    fn keyless_key_file_surfaces_no_private_key() {
        // A readable key file with no PEM private key must fail loudly, not
        // silently build a config without a key. Cert loading runs first and an
        // empty cert file yields an empty chain, so the error is NoPrivateKey.
        let dir = std::env::temp_dir();
        let cert = dir.join("ferro_tls_test_empty_cert.pem");
        let key = dir.join("ferro_tls_test_keyless.pem");
        std::fs::write(&cert, b"").expect("write temp cert");
        std::fs::write(&key, b"not a private key\n").expect("write temp key");
        let result = server_config(
            cert.to_str().expect("utf8 path"),
            key.to_str().expect("utf8 path"),
        );
        let _ = std::fs::remove_file(&cert);
        let _ = std::fs::remove_file(&key);
        assert!(matches!(result, Err(TlsError::NoPrivateKey)));
    }
}
