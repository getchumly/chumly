use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use quinn::crypto::rustls::QuicServerConfig;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

/// Loads the server's key and self-signed certificate, or generates them; the
/// flag is true if a new key was just created. Clients pin the fingerprint of
/// the key (`shared::key_fingerprint`) on first connect and derive users' keys
/// from it, so key.der must be kept between runs. The certificate is only a
/// wrapper of the key: if cert.der is missing, a new one is issued for the
/// same key and nothing changes for clients.
pub fn load_or_create(dir: &Path) -> anyhow::Result<(CertificateDer<'static>, PrivateKeyDer<'static>, bool)> {
    let cert_path = dir.join("cert.der");
    let key_path = dir.join("key.der");
    let pair =
        |cert: Vec<u8>, key: Vec<u8>| (CertificateDer::from(cert), PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)));
    if key_path.exists() {
        let key = std::fs::read(&key_path).context("reading key.der")?;
        if cert_path.exists() {
            let cert = std::fs::read(&cert_path).context("reading cert.der")?;
            let (cert, key) = pair(cert, key);
            return Ok((cert, key, false));
        }
        let key_pair = rcgen::KeyPair::try_from(key.as_slice()).context("key.der is damaged")?;
        let cert = issue_cert(&key_pair)?;
        shared::fsutil::write_atomic(&cert_path, &cert).context("writing cert.der")?;
        tracing::warn!(
            "cert.der was missing: issued a new certificate for the existing key.der; clients and users' keys are not affected"
        );
        let (cert, key) = pair(cert, key);
        return Ok((cert, key, false));
    }
    if cert_path.exists() {
        anyhow::bail!(
            "{} exists but key.der does not: the server key is lost. With a new key every client sees a changed \
             certificate and every user gets a new key (roles, registrations and bans by key stop matching). \
             Restore key.der from a backup, or delete cert.der to create a new key.",
            cert_path.display()
        );
    }
    let key_pair = rcgen::KeyPair::generate().context("generating the server key")?;
    let key = key_pair.serialize_der();
    // The key first: if the server stops between the two writes, the next
    // start issues the certificate for it (above).
    shared::fsutil::write_private(&key_path, &key).context("writing key.der")?;
    let cert = issue_cert(&key_pair)?;
    shared::fsutil::write_atomic(&cert_path, &cert).context("writing cert.der")?;
    tracing::info!("created a new server key and certificate: {}", key_path.display());
    let (cert, key) = pair(cert, key);
    Ok((cert, key, true))
}

/// A self-signed certificate for the server's key.
fn issue_cert(key_pair: &rcgen::KeyPair) -> anyhow::Result<Vec<u8>> {
    let names = vec![shared::TLS_SERVER_NAME.to_string(), "localhost".to_string()];
    let cert = rcgen::CertificateParams::new(names)?.self_signed(key_pair).context("creating the certificate")?;
    Ok(cert.der().to_vec())
}

/// `public_status`: also accept status queries (`shared::STATUS_ALPN`).
pub fn server_config(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    public_status: bool,
) -> anyhow::Result<quinn::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyClientKey { provider: provider.clone() }))
        .with_single_cert(vec![cert], key)?;
    // Every protocol major this build speaks; the client's newest match wins.
    tls.alpn_protocols = shared::alpn_protocols();
    if public_status {
        tls.alpn_protocols.push(shared::STATUS_ALPN.to_vec());
    }
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.max_idle_timeout(Some(Duration::from_secs(20).try_into()?));
    // The protocol uses exactly one client-opened bidirectional stream.
    // Refusing more keeps a client from tying up memory with extra streams.
    transport.max_concurrent_bidi_streams(1u32.into());
    transport.max_concurrent_uni_streams(0u32.into());
    cfg.transport_config(Arc::new(transport));
    Ok(cfg)
}

/// Client certificates are optional and self-signed: the certificate is
/// only a container for the user's public key, which identifies them (see
/// `client_key`). No chain or dates are checked, but the handshake signature
/// is, so a client can only present a key it actually holds.
#[derive(Debug)]
struct AnyClientKey {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClientKey {
    fn offer_client_auth(&self) -> bool {
        true
    }

    /// Clients without a key (unreadable identity file) are still let in.
    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        shared::key_fingerprint(end_entity)
            .ok_or_else(|| rustls::Error::General("malformed client certificate".into()))?;
        Ok(ClientCertVerified::assertion())
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

/// The user's key: SHA-256 of the public key (SPKI) from the client
/// certificate, as "AB:CD:...". `None` for clients without a certificate.
/// The certificate itself changes on every connection; the key does not.
pub fn client_key(conn: &quinn::Connection) -> Option<String> {
    let certs = conn.peer_identity()?.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    shared::key_fingerprint(certs.first()?)
}

/// First 8 bytes of a key fingerprint, for logs and the console.
pub fn short_key(key: &str) -> &str {
    key.get(..23).unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint_in(dir: &Path) -> String {
        let (cert, _, _) = load_or_create(dir).unwrap();
        shared::key_fingerprint(&cert).unwrap()
    }

    #[test]
    fn creates_then_loads_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, _, created) = load_or_create(dir.path()).unwrap();
        assert!(created);
        assert!(!dir.path().join("key.der.tmp").exists());
        let (again, _, created) = load_or_create(dir.path()).unwrap();
        assert!(!created);
        assert_eq!(cert, again);
    }

    /// A lost or damaged cert.der is not a new server: the certificate is
    /// issued again for the same key, the fingerprint clients pin stays.
    #[test]
    fn missing_cert_is_reissued_for_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let before = fingerprint_in(dir.path());
        let old_cert = std::fs::read(dir.path().join("cert.der")).unwrap();
        std::fs::remove_file(dir.path().join("cert.der")).unwrap();

        let (cert, key, created) = load_or_create(dir.path()).unwrap();
        assert!(!created, "not a new key");
        assert_eq!(shared::key_fingerprint(&cert).unwrap(), before);
        assert_ne!(cert.as_ref(), old_cert.as_slice(), "a new certificate");
        assert_eq!(std::fs::read(dir.path().join("cert.der")).unwrap(), cert.as_ref());
        // The pair is usable: TLS accepts the new certificate with the old key.
        server_config(cert, key, false).unwrap();
    }

    /// Without key.der the server must not silently make a new key: that
    /// would change every user's key on it.
    #[test]
    fn missing_key_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        load_or_create(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join("key.der")).unwrap();
        let err = load_or_create(dir.path()).unwrap_err().to_string();
        assert!(err.contains("key.der does not"), "{err}");
        assert!(dir.path().join("cert.der").exists(), "cert.der is left alone");
    }
}
