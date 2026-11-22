//! Connecting: resolving the address, the QUIC/TLS handshake (with a
//! probe handshake on the first connection), Hello and the server's answer.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::QuicClientConfig;
use shared::framing::{read_msg, write_msg};
use shared::media::MediaKind;
use shared::{ClientMsg, ServerMsg};
use tokio::time::timeout;

use super::tls::{self, ClientCert};
use super::{ConnectParams, DisconnectReason};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// TLS alert `no_application_protocol`: the server speaks no protocol major
/// this client offered.
pub(super) const NO_APPLICATION_PROTOCOL: u8 = 120;

/// A logged-in connection: the server answered Hello with Welcome.
pub(super) struct Session {
    pub(super) endpoint: quinn::Endpoint,
    pub(super) conn: quinn::Connection,
    pub(super) send: quinn::SendStream,
    pub(super) recv: quinn::RecvStream,
    pub(super) welcome: ServerMsg,
    pub(super) fingerprint: String,
}

pub(super) enum ConnectError {
    /// Retrying is pointless: server rejection or a foreign certificate.
    Fatal(DisconnectReason),
    Retry(String),
}

fn retry(e: &dyn std::fmt::Display) -> ConnectError {
    ConnectError::Retry(e.to_string())
}

pub(super) async fn connect(p: &ConnectParams) -> Result<Session, ConnectError> {
    let addr = resolve(&p.address).await.map_err(ConnectError::Retry)?;
    let endpoint = tls::endpoint_for(addr).map_err(|e| retry(&e))?;

    // The client certificate is derived from the server's fingerprint, but
    // TLS asks for it before the server shows its certificate. On the very
    // first connection to a server the fingerprint is not known yet: learn
    // it with a handshake without a certificate, then connect again.
    let mut expected = p.fingerprint.clone();
    if expected.is_none() && p.identity.is_some() {
        let (probe, fp) = handshake(&endpoint, addr, None, None).await?;
        probe.close(0u32.into(), b"probe");
        expected = Some(fp);
    }
    let client_cert = match (&p.identity, &expected) {
        (Some(id), Some(fp)) => id.client_cert(fp).inspect_err(|e| tracing::warn!("no client certificate: {e:#}")).ok(),
        _ => None,
    };
    let (conn, fingerprint) = handshake(&endpoint, addr, expected, client_cert).await?;

    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| retry(&e))?;
    let hello = ClientMsg::Hello {
        client_version: shared::VERSION.to_string(),
        features: shared::feature_names(),
        nickname: p.nickname.clone(),
        password: p.password.clone(),
        // `Screen` means "can watch screen shares": every platform can.
        capabilities: vec![MediaKind::Voice, MediaKind::Screen],
    };
    write_msg(&mut send, &hello).await.map_err(|e| retry(&e))?;
    let first = timeout(CONNECT_TIMEOUT, read_msg::<_, ServerMsg>(&mut recv))
        .await
        .map_err(|_| ConnectError::Retry("server did not respond".into()))?
        .map_err(|e| retry(&e))?;
    match first {
        Some(welcome @ ServerMsg::Welcome { .. }) => Ok(Session { endpoint, conn, send, recv, welcome, fingerprint }),
        Some(ServerMsg::Rejected { reason }) => Err(ConnectError::Fatal(DisconnectReason::Rejected(reason))),
        Some(_) => Err(ConnectError::Retry("unexpected server response".into())),
        None => Err(ConnectError::Retry("server closed the connection".into())),
    }
}

/// QUIC/TLS handshake. Returns the connection and the server's
/// certificate fingerprint.
pub(super) async fn handshake(
    endpoint: &quinn::Endpoint,
    addr: SocketAddr,
    expected: Option<String>,
    client_cert: Option<ClientCert>,
) -> Result<(quinn::Connection, String), ConnectError> {
    // Every protocol major this build speaks, newest first; the server picks.
    let (tls, seen) =
        tls::client_config(expected.clone(), client_cert, shared::alpn_protocols()).map_err(|e| retry(&e))?;
    let crypto = QuicClientConfig::try_from(tls).map_err(|e| retry(&e))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.max_idle_timeout(Some(Duration::from_secs(15).try_into().expect("valid timeout")));
    cfg.transport_config(Arc::new(transport));

    let connecting = endpoint.connect_with(cfg, addr, shared::TLS_SERVER_NAME).map_err(|e| retry(&e))?;
    let conn = match timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => return Err(ConnectError::Retry(format!("{addr}: no response"))),
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            if let Some((saved, actual)) = tls::changed_cert(expected, &seen) {
                return Err(ConnectError::Fatal(DisconnectReason::CertMismatch { saved, actual }));
            }
            if is_incompatible(&e) {
                return Err(ConnectError::Fatal(DisconnectReason::Incompatible));
            }
            return Err(retry(&e));
        }
    };
    let fingerprint = seen.lock().unwrap().clone().unwrap_or_default();
    tracing::debug!("protocol major {:?}", negotiated_major(&conn));
    Ok((conn, fingerprint))
}

/// The protocol major the server picked from our ALPN list.
pub(super) fn negotiated_major(conn: &quinn::Connection) -> Option<u32> {
    conn.handshake_data()
        .and_then(|d| d.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
        .and_then(|d| d.protocol)
        .and_then(|p| shared::major_from_alpn(&p))
}

/// The server refused the handshake because no protocol major matched.
pub(super) fn is_incompatible(e: &quinn::ConnectionError) -> bool {
    let code = quinn::TransportErrorCode::crypto(NO_APPLICATION_PROTOCOL);
    match e {
        quinn::ConnectionError::ConnectionClosed(c) => c.error_code == code,
        quinn::ConnectionError::TransportError(t) => t.code == code,
        _ => false,
    }
}

/// "host", "host:port", "[v6]:port" or a bare IPv6 address; without a port
/// the default one.
pub(super) async fn resolve(address: &str) -> Result<SocketAddr, String> {
    let address = address.trim();
    // No port: "host" or an IPv6 address without brackets.
    let with_port = if address.starts_with('[') || address.matches(':').count() == 1 {
        address.to_string()
    } else if address.contains(':') {
        format!("[{address}]:{}", shared::DEFAULT_PORT)
    } else {
        format!("{address}:{}", shared::DEFAULT_PORT)
    };
    let addrs: Vec<SocketAddr> =
        tokio::net::lookup_host(&with_port).await.map_err(|e| format!("{with_port}: {e}"))?.collect();
    pick_address(&addrs).ok_or_else(|| format!("{with_port}: address not found"))
}

/// IPv4 first: the server listens on `0.0.0.0` (IPv4 only) by default, and
/// names such as `localhost` resolve to `::1` first, which would not connect.
fn pick_address(addrs: &[SocketAddr]) -> Option<SocketAddr> {
    addrs.iter().find(|a| a.is_ipv4()).or_else(|| addrs.first()).copied()
}

#[cfg(test)]
mod tests {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    use super::*;

    #[test]
    fn ipv4_is_preferred() {
        let v6: SocketAddr = "[::1]:7777".parse().unwrap();
        let v4: SocketAddr = "127.0.0.1:7777".parse().unwrap();
        assert_eq!(pick_address(&[v6, v4]), Some(v4));
        assert_eq!(pick_address(&[v6]), Some(v6), "an IPv6-only server still works");
        assert_eq!(pick_address(&[]), None);
    }

    /// A server that speaks only a protocol major this client does not know:
    /// the handshake fails with `Incompatible`, not with a retryable error.
    #[tokio::test]
    async fn no_common_protocol_major_is_incompatible() {
        let cert = rcgen::generate_simple_self_signed(vec![shared::TLS_SERVER_NAME.to_string()]).unwrap();
        let der = CertificateDer::from(cert.cert.der().to_vec());
        let key = PrivateKeyDer::try_from(cert.key_pair.serialize_der()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der], key)
            .unwrap();
        tls.alpn_protocols = vec![b"chumly/99".to_vec()];
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap();
        let server =
            quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(crypto)), "127.0.0.1:0".parse().unwrap())
                .unwrap();
        let addr = server.local_addr().unwrap();
        let accept = tokio::spawn(async move { server.accept().await.unwrap().await });

        let client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let result = handshake(&client, addr, None, None).await;
        assert!(
            matches!(result, Err(ConnectError::Fatal(DisconnectReason::Incompatible))),
            "{:?}",
            result.err().map(|e| match e {
                ConnectError::Fatal(r) => format!("fatal {r:?}"),
                ConnectError::Retry(s) => format!("retry {s}"),
            })
        );
        // The server sees the same TLS alert (the server logs it as an
        // incompatible client).
        let server_side = accept.await.unwrap();
        let code = quinn::TransportErrorCode::crypto(NO_APPLICATION_PROTOCOL);
        assert!(
            matches!(&server_side, Err(quinn::ConnectionError::TransportError(e)) if e.code == code),
            "{server_side:?}"
        );
    }
}
