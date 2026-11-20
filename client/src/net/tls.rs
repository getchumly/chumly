//! TLS for both connections (login and status): trust on first use of the
//! server's self-signed certificate.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

pub(super) type ClientCert = (CertificateDer<'static>, PrivateKeyDer<'static>);

/// Where the verifier puts the fingerprint of the certificate it saw.
pub(super) type SeenFingerprint = Arc<Mutex<Option<String>>>;

/// TLS 1.3 client config that checks the server with `TofuVerifier`
/// against `expected`, presents `client_cert` if any and offers `alpn`.
pub(super) fn client_config(
    expected: Option<String>,
    client_cert: Option<ClientCert>,
    alpn: Vec<Vec<u8>>,
) -> Result<(rustls::ClientConfig, SeenFingerprint), rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let seen = SeenFingerprint::default();
    let verifier = Arc::new(TofuVerifier { expected, seen: seen.clone(), provider: provider.clone() });
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let mut tls = match client_cert {
        Some((cert, key)) => builder.with_client_auth_cert(vec![cert], key)?,
        None => builder.with_no_client_auth(),
    };
    tls.alpn_protocols = alpn;
    Ok((tls, seen))
}

/// After a failed handshake: the saved fingerprint and the one the server
/// showed, if they differ.
pub(super) fn changed_cert(expected: Option<String>, seen: &SeenFingerprint) -> Option<(String, String)> {
    let actual = seen.lock().unwrap().clone()?;
    let saved = expected?;
    (!saved.eq_ignore_ascii_case(&actual)).then_some((saved, actual))
}

/// A client endpoint on any local port of the address family of `addr`.
pub(super) fn endpoint_for(addr: SocketAddr) -> std::io::Result<quinn::Endpoint> {
    let bind: SocketAddr = if addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse().expect("valid addr");
    quinn::Endpoint::client(bind)
}

/// Trust on first use (TOFU), like SSH. The server's self-signed
/// certificate is accepted if the fingerprint of its public key
/// (`shared::key_fingerprint`) matches the saved one or if none is saved yet.
#[derive(Debug)]
struct TofuVerifier {
    expected: Option<String>,
    seen: SeenFingerprint,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // The server's public key, not the whole certificate: a certificate
        // reissued with the same key is the same server, and users' keys are
        // derived from this value (see `identity`).
        let fp = shared::key_fingerprint(end_entity.as_ref())
            .ok_or_else(|| rustls::Error::General("cannot read the server certificate".into()))?;
        *self.seen.lock().unwrap() = Some(fp.clone());
        match &self.expected {
            Some(e) if !e.eq_ignore_ascii_case(&fp) => {
                Err(rustls::Error::General("certificate fingerprint mismatch".into()))
            }
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
