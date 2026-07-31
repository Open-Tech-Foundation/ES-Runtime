//! Shared server-side TLS setup for the listening providers.
//!
//! `runtime:net` `listen` and `runtime:http` `serve` both terminate TLS on
//! accept, from the same inputs — a PEM certificate chain, a PEM private key,
//! and an ALPN list. Building the `rustls` config is exacting enough (explicit
//! crypto provider, key formats, empty-chain handling) that having two copies
//! would mean two things to keep right, so it lives here once.

use std::sync::Arc;

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::crypto::aws_lc_rs;

fn err(e: impl ToString) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::Tls,
        message: e.to_string(),
    }
}

/// A server-side TLS acceptor presenting `cert` (a PEM chain, leaf first) with
/// `key` (a PEM private key) and advertising `alpn`.
///
/// Built once per bind, so the cert/key parse and config assembly are paid at
/// bind time rather than per accept. `aws_lc_rs` is selected explicitly: leaving
/// the process-default crypto provider ambiguous makes rustls panic at the first
/// handshake, which is not a failure mode worth inheriting.
pub(crate) fn server_acceptor(
    cert: &[u8],
    key: &[u8],
    alpn: &[String],
) -> Result<TlsAcceptor, ProviderError> {
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let certs = CertificateDer::pem_slice_iter(cert)
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    if certs.is_empty() {
        return Err(err("no certificates found in the PEM cert"));
    }
    let key = PrivateKeyDer::from_pem_slice(key).map_err(err)?;
    let provider = Arc::new(aws_lc_rs::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(err)?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(err)?;
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    Ok(TlsAcceptor::from(Arc::new(config)))
}
