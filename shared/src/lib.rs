//! Code shared by client and server: protocol, message framing,
//! media datagram format.

pub mod framing;
pub mod fsutil;
pub mod media;
pub mod protocol;
pub mod release;
pub mod wire;

pub use protocol::*;

use sha2::{Digest, Sha256};

/// SHA-256 of `der` formatted as "AB:CD:...".
pub fn fingerprint(der: &[u8]) -> String {
    let hash = Sha256::digest(der);
    hash.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Fingerprint of the public key (SPKI) in a DER certificate, not of the
/// certificate itself: a certificate reissued with the same key (new dates,
/// names, serial) keeps it. Both sides are identified this way: the server
/// by clients (TOFU pin, and the input of users' key derivation), users by
/// the server. `None` if the certificate cannot be parsed.
pub fn key_fingerprint(cert_der: &[u8]) -> Option<String> {
    let der = rustls::pki_types::CertificateDer::from(cert_der);
    let cert = webpki::EndEntityCert::try_from(&der).ok()?;
    Some(fingerprint(cert.subject_public_key_info().as_ref()))
}

/// Current Unix time in seconds.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-signed certificate made by `rcgen` for the tests.
    fn cert(key: &rcgen::KeyPair, name: &str) -> Vec<u8> {
        rcgen::CertificateParams::new(vec![name.to_string()]).unwrap().self_signed(key).unwrap().der().to_vec()
    }

    #[test]
    fn key_fingerprint_survives_reissue() {
        let key = rcgen::KeyPair::generate().unwrap();
        let a = cert(&key, "chumly");
        let b = cert(&key, "other-name");
        assert_ne!(fingerprint(&a), fingerprint(&b), "different certificates");
        assert_eq!(key_fingerprint(&a), key_fingerprint(&b), "same key");
        assert_eq!(key_fingerprint(&a).unwrap().len(), 32 * 3 - 1);

        let other = rcgen::KeyPair::generate().unwrap();
        assert_ne!(key_fingerprint(&a), key_fingerprint(&cert(&other, "chumly")), "different key");
    }

    #[test]
    fn key_fingerprint_rejects_garbage() {
        assert_eq!(key_fingerprint(b"not a certificate"), None);
        assert_eq!(key_fingerprint(&[]), None);
    }
}
