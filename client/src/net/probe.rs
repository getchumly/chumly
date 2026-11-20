//! Status of a server for the start screen: a short connection before
//! login that reads the public status.

use std::sync::Arc;
use std::time::{Duration, Instant};

use quinn::crypto::rustls::QuicClientConfig;
use tokio::time::timeout;

use super::connect::{is_incompatible, resolve};
use super::tls;

const STATUS_TIMEOUT: Duration = Duration::from_secs(4);

/// What the start screen shows on a server card before connecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerProbe {
    /// The server answered the status query.
    Online { ping_ms: u32, status: shared::ServerStatus },
    /// The server is up but gave no counts: an older server, or its owner
    /// turned the public status off (no common ALPN, which still proves it is up).
    Up { ping_ms: u32 },
    /// The certificate differs from the saved one: a warning before connecting.
    CertChanged { actual: String },
    /// No answer.
    Offline,
}

/// Status query before login (`shared::STATUS_ALPN`): a separate short
/// connection without a client certificate. Checks the server certificate
/// against the saved fingerprint, if there is one.
pub async fn probe(address: String, expected: Option<String>) -> ServerProbe {
    let Ok(addr) = resolve(&address).await else { return ServerProbe::Offline };
    let Ok(endpoint) = tls::endpoint_for(addr) else { return ServerProbe::Offline };
    let Ok((tls, seen)) = tls::client_config(expected.clone(), None, vec![shared::STATUS_ALPN.to_vec()]) else {
        return ServerProbe::Offline;
    };
    let Ok(crypto) = QuicClientConfig::try_from(tls) else { return ServerProbe::Offline };
    let cfg = quinn::ClientConfig::new(Arc::new(crypto));
    let Ok(connecting) = endpoint.connect_with(cfg, addr, shared::TLS_SERVER_NAME) else { return ServerProbe::Offline };

    let started = Instant::now();
    let conn = match timeout(STATUS_TIMEOUT, connecting).await {
        Err(_) => return ServerProbe::Offline,
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            if let Some((_, actual)) = tls::changed_cert(expected, &seen) {
                return ServerProbe::CertChanged { actual };
            }
            // It answered the handshake, just not with a status.
            if is_incompatible(&e) {
                return ServerProbe::Up { ping_ms: started.elapsed().as_millis() as u32 };
            }
            return ServerProbe::Offline;
        }
    };
    let ping_ms = conn.rtt().as_millis() as u32;
    let read = async {
        let mut stream = conn.accept_uni().await.ok()?;
        let bytes = stream.read_to_end(shared::wire::MAX_STATUS_BYTES).await.ok()?;
        shared::wire::decode_status(&bytes).ok()
    };
    let result = match timeout(STATUS_TIMEOUT, read).await {
        Ok(Some(status)) => ServerProbe::Online { ping_ms, status },
        _ => ServerProbe::Up { ping_ms },
    };
    conn.close(0u32.into(), b"done");
    result
}
