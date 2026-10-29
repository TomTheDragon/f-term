//! TLS setup shared by the websocket and the HTTP client.
//!
//! The crypto backend is picked at compile time: `ring` (fast, needs SSE2 on
//! 32-bit x86) or a pure-Rust RustCrypto provider (for i586 / Windows 9x).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};

use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();

#[cfg(feature = "tls-ring")]
fn provider() -> rustls::crypto::CryptoProvider {
    rustls::crypto::ring::default_provider()
}

#[cfg(all(not(feature = "tls-ring"), feature = "tls-rustcrypto"))]
fn provider() -> rustls::crypto::CryptoProvider {
    rustls_rustcrypto::provider()
}

#[cfg(not(any(feature = "tls-ring", feature = "tls-rustcrypto")))]
compile_error!("enable one TLS backend: feature `tls-ring` or `tls-rustcrypto`");

/// Fill `buf` from the TLS backend's random number source (for `--diagnose`).
pub fn fill_random(buf: &mut [u8]) -> Result<(), rustls::crypto::GetRandomFailed> {
    provider().secure_random.fill(buf)
}

pub fn backend_name() -> &'static str {
    if cfg!(feature = "tls-ring") {
        "ring"
    } else {
        "rustcrypto"
    }
}

pub fn config() -> Arc<ClientConfig> {
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let cfg = ClientConfig::builder_with_provider(Arc::new(provider()))
                .with_safe_default_protocol_versions()
                .expect("TLS protocol versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(cfg)
        })
        .clone()
}

pub type TlsStream<S> = StreamOwned<ClientConnection, S>;

/// Wrap a stream in a TLS client session for `host`.
pub fn wrap<S: Read + Write>(host: &str, sock: S) -> std::io::Result<TlsStream<S>> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
    let conn = ClientConnection::new(config(), name).map_err(std::io::Error::other)?;
    Ok(StreamOwned::new(conn, sock))
}

/// Adapter so ureq can use our rustls setup without its own TLS feature.
pub struct UreqTls;

struct UreqStream(TlsStream<Box<dyn ureq::ReadWrite>>);

impl std::fmt::Debug for UreqStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UreqStream")
    }
}

impl Read for UreqStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.0.read(buf) {
            // Some servers close without close_notify; treat as EOF.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
            r => r,
        }
    }
}

impl Write for UreqStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl ureq::ReadWrite for UreqStream {
    fn socket(&self) -> Option<&TcpStream> {
        self.0.sock.socket()
    }
}

impl ureq::TlsConnector for UreqTls {
    fn connect(&self, dns_name: &str, io: Box<dyn ureq::ReadWrite>) -> Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
        let s = wrap(dns_name, io)?;
        Ok(Box::new(UreqStream(s)))
    }
}
