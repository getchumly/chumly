//! User identity. One random secret per installation; from it and the
//! fingerprint of a server's public key (`shared::key_fingerprint`) a separate
//! Ed25519 key is derived for every server. The client presents that key as a TLS client certificate,
//! and the server recognizes the user by its public key. Different servers
//! see unrelated keys and cannot tell they belong to the same person, yet
//! backing up the one secret restores the identity on all of them.
//!
//! The secret is stored as a recovery code in identity.txt next to
//! client.toml.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

pub const FILE_NAME: &str = "identity.txt";
const SECRET_LEN: usize = 32;
const CHECKSUM_LEN: usize = 2;
/// The first byte of a recovery code: which scheme turns the secret into
/// keys (the secret length, `KDF_SALT`, what the server key fingerprint is).
/// A code with a version this build does not know is refused as "from a newer
/// version", not as a typo. A future scheme gets a new number, and codes of
/// the old ones keep working.
const CODE_VERSION: u8 = 1;
/// Version, secret, checksum: 35 bytes are exactly 56 base32 characters,
/// 14 groups of four.
const CODE_BYTES: usize = 1 + SECRET_LEN + CHECKSUM_LEN;
/// Crockford base32: no I, L, O, U, so the code survives being written by hand.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Changing this changes every derived key: do not.
const KDF_SALT: &[u8] = b"chumly identity v1";
/// PKCS#8 v1 header for an Ed25519 private key; the 32-byte seed follows.
const ED25519_PKCS8_PREFIX: [u8; 16] =
    [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];

const FILE_HEADER: &str = "\
# Chumly identity: your recovery code.
# Servers recognize you by keys derived from it. Anyone who has this code can
# sign in as you on every server, so keep it secret. Keep a copy somewhere safe:
# without it, your registered nickname and rights on servers cannot be restored.
";

pub struct Identity {
    secret: [u8; SECRET_LEN],
}

/// Why a recovery code was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    /// A typo, a wrong length or not a code at all.
    Invalid,
    /// A valid code of a scheme this build does not know.
    NewerVersion,
}

impl std::fmt::Display for CodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "the recovery code is damaged",
            Self::NewerVersion => "the recovery code is from a newer version of Chumly, update the app",
        })
    }
}

impl std::error::Error for CodeError {}

/// Never prints the secret.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Identity(..)")
    }
}

impl Identity {
    pub fn generate() -> anyhow::Result<Self> {
        let mut secret = [0u8; SECRET_LEN];
        SystemRandom::new().fill(&mut secret).map_err(|_| anyhow::anyhow!("system random generator failed"))?;
        Ok(Self { secret })
    }

    /// A fixed identity for tests.
    #[cfg(test)]
    pub fn from_secret(secret: [u8; SECRET_LEN]) -> Self {
        Self { secret }
    }

    /// identity.txt in the same folder as the given client.toml.
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_file_name(FILE_NAME)
    }

    /// Loads the identity, creating a new one if the file does not exist.
    /// A file that exists but cannot be read is an error: it must never be
    /// overwritten silently, that would lose the user's identity.
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let code = text
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && !l.starts_with('#'))
                    .context("no recovery code in the file")?;
                Ok(Self::from_recovery_code(code)?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let id = Self::generate()?;
                id.save(path)?;
                tracing::info!("created a new identity: {}", path.display());
                Ok(id)
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Writes the identity atomically, readable by the current user only.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text = format!("{FILE_HEADER}{}\n", self.recovery_code());
        shared::fsutil::write_private(path, text.as_bytes()).with_context(|| format!("writing {}", path.display()))
    }

    /// Replaces the identity file, keeping the previous one as identity.txt.bak
    /// (as private as the identity itself: a plain copy would take the
    /// folder's permissions).
    pub fn replace_file(&self, path: &Path) -> anyhow::Result<()> {
        if path.exists() {
            let old = std::fs::read(path).context("reading the old identity")?;
            shared::fsutil::write_private(&path.with_extension("txt.bak"), &old)
                .context("backing up the old identity")?;
        }
        self.save(path)
    }

    /// The version, the secret and a checksum of both in base32, grouped by
    /// four: "XXXX-XXXX-...-XXXX", 56 characters without the dashes.
    pub fn recovery_code(&self) -> String {
        let mut payload = vec![CODE_VERSION];
        payload.extend_from_slice(&self.secret);
        payload.extend_from_slice(&checksum(&payload));
        let chars = base32_encode(&payload);
        chars.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).expect("ascii")).collect::<Vec<_>>().join("-")
    }

    /// Parses a recovery code. Case, dashes and spaces do not matter;
    /// O is read as 0 and I, L as 1.
    pub fn from_recovery_code(code: &str) -> Result<Self, CodeError> {
        let payload = base32_decode(code).ok_or(CodeError::Invalid)?;
        if payload.len() != CODE_BYTES {
            return Err(CodeError::Invalid);
        }
        let (body, sum) = payload.split_at(CODE_BYTES - CHECKSUM_LEN);
        // The checksum first: a typo in the version byte is a typo.
        if sum != checksum(body) {
            return Err(CodeError::Invalid);
        }
        match body[0] {
            CODE_VERSION => Ok(Self { secret: body[1..].try_into().map_err(|_| CodeError::Invalid)? }),
            0 => Err(CodeError::Invalid),
            _ => Err(CodeError::NewerVersion),
        }
    }

    /// Ed25519 seed for the server with this key fingerprint.
    fn seed_for(&self, server_fingerprint: &str) -> anyhow::Result<[u8; 32]> {
        let fp = parse_fingerprint(server_fingerprint)?;
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, KDF_SALT).extract(&self.secret);
        let info = [fp.as_slice()];
        let okm = prk.expand(&info, hkdf::HKDF_SHA256).map_err(|_| anyhow::anyhow!("key derivation failed"))?;
        let mut seed = [0u8; 32];
        okm.fill(&mut seed).map_err(|_| anyhow::anyhow!("key derivation failed"))?;
        Ok(seed)
    }

    /// TLS client certificate and key for the server with this key
    /// fingerprint. The certificate itself is regenerated every time (new
    /// serial and dates); the server identifies the user by the public key.
    pub fn client_cert(
        &self,
        server_fingerprint: &str,
    ) -> anyhow::Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        let seed = self.seed_for(server_fingerprint)?;
        let mut pkcs8 = ED25519_PKCS8_PREFIX.to_vec();
        pkcs8.extend_from_slice(&seed);
        let key_der = PrivatePkcs8KeyDer::from(pkcs8);
        let key_pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&key_der, &rcgen::PKCS_ED25519)
            .context("loading the derived key")?;
        let cert = rcgen::CertificateParams::new(vec![shared::TLS_CLIENT_NAME.to_string()])?
            .self_signed(&key_pair)
            .context("creating the client certificate")?;
        Ok((cert.der().clone(), PrivateKeyDer::Pkcs8(key_der)))
    }
}

fn checksum(data: &[u8]) -> [u8; CHECKSUM_LEN] {
    let h = Sha256::digest(data);
    [h[0], h[1]]
}

/// "AB:CD:..." (any case, colons optional) to 32 bytes.
fn parse_fingerprint(fp: &str) -> anyhow::Result<Vec<u8>> {
    let hex: Vec<u8> = fp.bytes().filter(|b| *b != b':').collect();
    if hex.len() != 64 {
        bail!("invalid server fingerprint");
    }
    hex.chunks(2)
        .map(|p| {
            let s = std::str::from_utf8(p).ok()?;
            u8::from_str_radix(s, 16).ok()
        })
        .collect::<Option<Vec<u8>>>()
        .context("invalid server fingerprint")
}

fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in data {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' | '\t' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        let v = ALPHABET.iter().position(|&a| a as char == c)? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    // Leftover padding bits must be zero, otherwise the code was mistyped.
    (acc == 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    const FP_A: &str =
        "CE:47:F7:14:F3:F3:6C:27:D4:35:6C:3D:EF:0D:53:FF:36:46:17:25:31:B4:B8:56:6D:3E:E9:54:43:6C:1C:70";
    const FP_B: &str =
        "00:47:F7:14:F3:F3:6C:27:D4:35:6C:3D:EF:0D:53:FF:36:46:17:25:31:B4:B8:56:6D:3E:E9:54:43:6C:1C:70";

    fn public_key(id: &Identity, fp: &str) -> Vec<u8> {
        let seed = id.seed_for(fp).unwrap();
        Ed25519KeyPair::from_seed_unchecked(&seed).unwrap().public_key().as_ref().to_vec()
    }

    #[test]
    fn recovery_code_roundtrip() {
        let id = Identity::generate().unwrap();
        let code = id.recovery_code();
        // 14 full groups of four.
        assert_eq!(code.len(), 56 + 13);
        assert!(code.split('-').all(|g| g.len() == 4), "{code}");
        let back = Identity::from_recovery_code(&code).unwrap();
        assert_eq!(back.secret, id.secret);
        // Lowercase, no dashes, spaces: still fine.
        let sloppy = code.to_lowercase().replace('-', " ");
        assert_eq!(Identity::from_recovery_code(&sloppy).unwrap().secret, id.secret);
    }

    /// The format is fixed: this code must always give this secret.
    #[test]
    fn recovery_code_format_is_stable() {
        let id = Identity::from_secret([7; SECRET_LEN]);
        let code = id.recovery_code();
        assert!(code.starts_with("04"), "the version byte 1 comes first: {code}");
        assert_eq!(Identity::from_recovery_code(&code).unwrap().secret, [7; SECRET_LEN]);
        assert_eq!(code, "043G-E1R7-0W3G-E1R7-0W3G-E1R7-0W3G-E1R7-0W3G-E1R7-0W3G-E1R7-0W3G-FMWG");
    }

    #[test]
    fn recovery_code_detects_typos() {
        let code = Identity::generate().unwrap().recovery_code();
        // Every position, the version included.
        for i in [0, 1, 7, code.len() - 1] {
            let mut chars: Vec<char> = code.chars().collect();
            chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
            let typo: String = chars.into_iter().collect();
            assert_eq!(Identity::from_recovery_code(&typo).unwrap_err(), CodeError::Invalid, "position {i}");
        }
        assert_eq!(Identity::from_recovery_code(&code[..code.len() - 1]).unwrap_err(), CodeError::Invalid);
        assert_eq!(Identity::from_recovery_code("").unwrap_err(), CodeError::Invalid);
        assert_eq!(Identity::from_recovery_code("UUUU").unwrap_err(), CodeError::Invalid);
    }

    /// A code of the format before versions (55 characters) is not accepted.
    #[test]
    fn unversioned_code_is_refused() {
        let secret = [7u8; SECRET_LEN];
        let mut payload = secret.to_vec();
        payload.extend_from_slice(&checksum(&secret));
        let old = base32_encode(&payload);
        assert_eq!(old.len(), 55);
        assert_eq!(Identity::from_recovery_code(&old).unwrap_err(), CodeError::Invalid);
    }

    /// A valid code of an unknown version says so instead of "typo".
    #[test]
    fn newer_version_is_reported() {
        let code_with = |version: u8| {
            let mut payload = vec![version];
            payload.extend_from_slice(&[7; SECRET_LEN]);
            payload.extend_from_slice(&checksum(&payload));
            base32_encode(&payload)
        };
        assert_eq!(Identity::from_recovery_code(&code_with(2)).unwrap_err(), CodeError::NewerVersion);
        assert_eq!(Identity::from_recovery_code(&code_with(0)).unwrap_err(), CodeError::Invalid);
        assert!(Identity::from_recovery_code(&code_with(CODE_VERSION)).is_ok());
    }

    #[test]
    fn keys_are_per_server_and_stable() {
        let id = Identity::generate().unwrap();
        assert_eq!(public_key(&id, FP_A), public_key(&id, FP_A));
        assert_eq!(public_key(&id, FP_A), public_key(&id, &FP_A.to_lowercase()));
        assert_ne!(public_key(&id, FP_A), public_key(&id, FP_B));
        let other = Identity::generate().unwrap();
        assert_ne!(public_key(&id, FP_A), public_key(&other, FP_A));
        assert!(id.seed_for("AB:CD").is_err());
    }

    #[test]
    fn client_cert_is_usable() {
        let id = Identity::generate().unwrap();
        let (cert, key) = id.client_cert(FP_A).unwrap();
        assert!(!cert.as_ref().is_empty());
        rustls::crypto::ring::sign::any_supported_type(&key).unwrap();
    }

    #[test]
    fn file_roundtrip_and_damaged_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let id = Identity::load_or_create(&path).unwrap();
        let again = Identity::load_or_create(&path).unwrap();
        assert_eq!(id.secret, again.secret);

        std::fs::write(&path, "# header\nnot a code\n").unwrap();
        assert!(Identity::load_or_create(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# header\nnot a code\n");

        let new = Identity::generate().unwrap();
        new.replace_file(&path).unwrap();
        assert_eq!(Identity::load_or_create(&path).unwrap().secret, new.secret);
        assert!(path.with_extension("txt.bak").exists());
    }
}
