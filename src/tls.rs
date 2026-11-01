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

/// The built-in list of trusted root certificates (Mozilla's).
#[cfg(not(feature = "system-certs"))]
fn root_certs() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    roots
}

/// The system's trusted root certificates (feature `system-certs`, for
/// distribution packages that depend on ca-certificates).
#[cfg(feature = "system-certs")]
fn root_certs() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    for path in system_cert_files() {
        if let Ok(pem) = std::fs::read_to_string(&path) {
            let (added, _) = roots.add_parsable_certificates(pem_certificates(&pem));
            if added > 0 {
                break;
            }
        }
    }
    roots
}

/// Where Linux distributions keep their CA bundle; SSL_CERT_FILE first.
#[cfg(feature = "system-certs")]
fn system_cert_files() -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = std::env::var_os("SSL_CERT_FILE").into_iter().map(Into::into).collect();
    for p in [
        "/etc/ssl/certs/ca-certificates.crt",                // Debian, Ubuntu, Arch
        "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem", // Fedora, RHEL
        "/etc/pki/tls/certs/ca-bundle.crt",                  // older Fedora, RHEL
        "/etc/ssl/ca-bundle.pem",                            // openSUSE
        "/etc/ssl/cert.pem",                                 // Alpine, macOS, BSD
    ] {
        v.push(p.into());
    }
    v
}

/// The certificates in a PEM file.
#[cfg_attr(not(any(test, feature = "system-certs")), allow(dead_code))]
fn pem_certificates(pem: &str) -> Vec<rustls::pki_types::CertificateDer<'static>> {
    use base64::Engine as _;
    let mut out = Vec::new();
    let mut body: Option<String> = None;
    for line in pem.lines().map(str::trim) {
        if line == "-----BEGIN CERTIFICATE-----" {
            body = Some(String::new());
        } else if line == "-----END CERTIFICATE-----" {
            if let Some(Ok(der)) = body.take().map(|b| base64::engine::general_purpose::STANDARD.decode(b)) {
                out.push(der.into());
            }
        } else if let Some(b) = &mut body {
            b.push_str(line);
        }
    }
    out
}

/// How many trusted root certificates there are (0 = connections will fail).
pub fn root_count() -> usize {
    config();
    ROOTS.get().copied().unwrap_or(0)
}

static ROOTS: OnceLock<usize> = OnceLock::new();

pub fn config() -> Arc<ClientConfig> {
    CONFIG
        .get_or_init(|| {
            let roots = root_certs();
            ROOTS.get_or_init(|| roots.len());
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
    if root_count() == 0 {
        return Err(std::io::Error::other(
            "no trusted CA certificates found on this system (install the ca-certificates package, or set SSL_CERT_FILE)",
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pem_certificates() {
        let pem = "junk\n-----BEGIN CERTIFICATE-----\nAQID\nBA==\n-----END CERTIFICATE-----\n\
                   -----BEGIN PRIVATE KEY-----\nBQY=\n-----END PRIVATE KEY-----\n\
                   -----BEGIN CERTIFICATE-----\nBwg=\n-----END CERTIFICATE-----\n";
        let certs = pem_certificates(pem);
        assert_eq!(certs.len(), 2);
        assert_eq!(certs[0].as_ref(), &[1, 2, 3, 4]);
        assert_eq!(certs[1].as_ref(), &[7, 8]);
    }

    #[test]
    fn has_root_certificates() {
        // Built in, or from the system with `system-certs` (needs a CA bundle).
        assert!(root_count() > 50, "{}", root_count());
    }
}
