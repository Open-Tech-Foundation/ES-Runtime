//! Shared server-side TLS for the listening providers.
//!
//! `runtime:net` `listen` and `runtime:http` `serve` both terminate TLS on
//! accept, from the same inputs — a PEM certificate chain, a PEM private key,
//! an ALPN list, and for a server that requires client certificates (D201) the
//! CA they must chain to and the CRLs that revoke them (D202). Building the
//! `rustls` config is exacting enough (explicit crypto provider, key formats,
//! empty-chain handling) that having two copies would mean two things to keep
//! right, so it lives here once.
//!
//! The configuration is replaceable while the server runs (D200): every
//! accepted connection takes the one current when it arrives, so
//! `setSecureContext` changes what new handshakes present and leaves open
//! connections alone.

use std::sync::{Arc, Mutex, RwLock};

use es_runtime_common::ErrorCode;
use es_runtime_providers::{ProviderError, SecureContext};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::crypto::aws_lc_rs;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer,
};
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{RootCertStore, ServerConfig};

fn err(e: impl ToString) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::Tls,
        message: e.to_string(),
    }
}

/// What a TLS server is configured from. Kept whole so a partial
/// `setSecureContext` can be merged into it.
#[derive(Clone, Default)]
pub(crate) struct ServerTlsMaterial {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
    /// Client certificates must chain to this (D201). Empty ⇒ none requested.
    pub client_ca: Vec<u8>,
    /// PEM CRLs a client certificate is checked against (D202).
    pub crl: Vec<u8>,
    /// Whether a client certificate is required (`requestCert: true`).
    pub request_cert: bool,
}

/// A TLS server's current configuration, replaceable while it runs. Cheap to
/// clone: every clone is the same server.
#[derive(Clone)]
pub(crate) struct ServerTls {
    current: Arc<RwLock<TlsAcceptor>>,
    material: Arc<Mutex<ServerTlsMaterial>>,
    alpn: Arc<Vec<String>>,
}

impl ServerTls {
    /// Built at bind time, so a bad certificate fails the bind rather than
    /// every connection after the port is open.
    pub(crate) fn new(material: ServerTlsMaterial, alpn: &[String]) -> Result<Self, ProviderError> {
        let acceptor = acceptor(&material, alpn)?;
        Ok(ServerTls {
            current: Arc::new(RwLock::new(acceptor)),
            material: Arc::new(Mutex::new(material)),
            alpn: Arc::new(alpn.to_vec()),
        })
    }

    /// The acceptor for the connection arriving now.
    pub(crate) fn acceptor(&self) -> TlsAcceptor {
        self.current.read().unwrap().clone()
    }

    /// Replaces the configuration with `update` merged into it (D200). What
    /// `update` leaves out keeps its current value. Checked before anything
    /// changes: on an error the server keeps serving what it had.
    pub(crate) fn set(&self, update: SecureContext) -> Result<(), ProviderError> {
        let mut material = self.material.lock().unwrap();
        let mut next = material.clone();
        if let Some(cert) = update.cert {
            next.cert = cert;
        }
        if let Some(key) = update.key {
            next.key = key;
        }
        if let Some(ca) = update.ca {
            next.client_ca = ca;
        }
        if let Some(crl) = update.crl {
            next.crl = crl;
        }
        let acceptor = acceptor(&next, &self.alpn)?;
        *self.current.write().unwrap() = acceptor;
        *material = next;
        Ok(())
    }
}

/// A server-side TLS acceptor for `material`, advertising `alpn`.
///
/// `aws_lc_rs` is selected explicitly: leaving the process-default crypto
/// provider ambiguous makes rustls panic at the first handshake, which is not
/// a failure mode worth inheriting.
fn acceptor(material: &ServerTlsMaterial, alpn: &[String]) -> Result<TlsAcceptor, ProviderError> {
    let certs = CertificateDer::pem_slice_iter(&material.cert)
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    if certs.is_empty() {
        return Err(err("no certificates found in the PEM cert"));
    }
    let key = PrivateKeyDer::from_pem_slice(&material.key).map_err(err)?;
    let provider = Arc::new(aws_lc_rs::default_provider());
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(err)?;
    let builder = if material.request_cert {
        builder.with_client_cert_verifier(client_verifier(material, provider)?)
    } else {
        // Without `requestCert` these would do nothing, and a server whose
        // operator believes it checks client certificates and does not is the
        // failure worth refusing.
        if !material.client_ca.is_empty() || !material.crl.is_empty() {
            return Err(err(
                "ca and crl verify client certificates, which a server asks for only with \
                 requestCert: true",
            ));
        }
        builder.with_no_client_auth()
    };
    // Refuses a key that does not belong to the certificate.
    let mut config = builder.with_single_cert(certs, key).map_err(err)?;
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Verifies client certificates against the server's `ca` and `crl`.
fn client_verifier(
    material: &ServerTlsMaterial,
    provider: Arc<tokio_rustls::rustls::crypto::CryptoProvider>,
) -> Result<Arc<dyn tokio_rustls::rustls::server::danger::ClientCertVerifier>, ProviderError> {
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(&material.client_ca) {
        roots.add(cert.map_err(err)?).map_err(err)?;
    }
    if roots.is_empty() {
        return Err(err(
            "requestCert: true needs a ca: the CA client certificates must chain to",
        ));
    }
    let crls = crls(&material.crl)?;
    WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
        .with_crls(crls)
        .allow_unknown_revocation_status()
        .enforce_revocation_expiration()
        .build()
        .map_err(err)
}

/// The CRLs in a PEM `crl`. An empty value is none; a value with something in
/// it but no CRL is an error, so a typo does not silently disable revocation.
pub(crate) fn crls(
    pem: &[u8],
) -> Result<Vec<CertificateRevocationListDer<'static>>, ProviderError> {
    let crls = CertificateRevocationListDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    if crls.is_empty() && pem.iter().any(|b| !b.is_ascii_whitespace()) {
        return Err(err("crl holds no X509 CRL block"));
    }
    Ok(crls)
}
