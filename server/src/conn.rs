use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use bytes::Bytes;
use shared::framing::{read_msg, read_msg_limited, write_msg};
use shared::media::{self, MediaKind};
use shared::{ClientMsg, ErrorCode, RejectReason, ServerMsg, UserId};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::limits::TokenBucket;

/// TLS alert `no_application_protocol`: the client offered no protocol
/// major the server speaks.
const NO_APPLICATION_PROTOCOL: u8 = 120;
use crate::state::{After, Hello, Server};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Hello carries a nickname, a password and a short capability list:
/// a few kilobytes is plenty. Before login nobody gets to make us
/// allocate the full `MAX_FRAME_BYTES`.
const MAX_HELLO_BYTES: usize = 4 * 1024;
/// Delay before answering a wrong server password.
const WRONG_PASSWORD_DELAY: Duration = Duration::from_secs(1);
/// Messages queued for one client. A client that falls this far behind
/// is disconnected rather than buffered without limit.
const SEND_QUEUE: usize = 1024;
/// Clients split media into datagrams of at most `MAX_CLIENT_DATAGRAM`.
/// Anything larger is dropped.
const MAX_MEDIA_DATAGRAM: usize = media::MAX_CLIENT_DATAGRAM;

/// Control messages of any kind. A normal client sends a handful per
/// minute; exceeding this means a broken or hostile client: disconnect.
const CONTROL_BURST: f64 = 50.0;
const CONTROL_PER_SEC: f64 = 20.0;
/// Chat and private messages. Excess messages are dropped.
/// The client mirrors this limit (`ChatLimit` in client/src/app.rs).
const CHAT_BURST: f64 = 10.0;
const CHAT_PER_SEC: f64 = 1.0;
/// Channel switches. The heaviest message: every switch sends the channel
/// history to the user and an update to everyone. Excess switches are refused
/// with `RateLimited`; the client moves only when the server confirms.
const JOIN_BURST: f64 = 5.0;
const JOIN_PER_SEC: f64 = 1.0;
/// Media rates in bytes per second, with a per-datagram overhead so a
/// flood of tiny datagrams is also limited. Voice is Opus at 32 kbit/s
/// (about 8 KB/s with overhead); screen share is up to 6 Mbit/s video
/// with large keyframes plus 128 kbit/s audio. Excess datagrams are dropped.
const DATAGRAM_COST_OVERHEAD: f64 = 64.0;
const VOICE_BYTES_PER_SEC: f64 = 32_000.0;
const VOICE_BURST_BYTES: f64 = 64_000.0;
const SCREEN_BYTES_PER_SEC: f64 = 2_000_000.0;
const SCREEN_BURST_BYTES: f64 = 2_000_000.0;

pub async fn handle(server: Arc<Server>, incoming: quinn::Incoming) {
    let addr = incoming.remote_address();
    // Make the client prove it owns its address (stateless retry, one extra
    // round trip) before counting it: otherwise spoofed packets could use up
    // the connection slots of somebody else's address.
    if !incoming.remote_address_validated() && incoming.may_retry() {
        if let Err(e) = incoming.retry() {
            debug!("{addr}: retry failed: {e}");
        }
        return;
    }
    let max_per_ip = server.lock().config.max_connections_per_ip;
    let Some(_slot) = server.conns.try_acquire(addr.ip(), max_per_ip) else {
        info!("{addr}: refused, too many connections from this address (max_connections_per_ip = {max_per_ip})");
        incoming.refuse();
        return;
    };
    let Ok(pending) = server.pending.clone().try_acquire_owned() else {
        warn!("{addr}: refused, too many connections logging in at once");
        incoming.refuse();
        return;
    };
    let conn = match incoming.await {
        Ok(c) if negotiated(&c).as_deref() == Some(shared::STATUS_ALPN) => {
            serve_status(&server, c, addr).await;
            return;
        }
        Ok(c) => c,
        // No protocol major in common (see `shared::PROTOCOL_MAJORS`): a client
        // much older or newer than the server. Worth an info line.
        Err(quinn::ConnectionError::TransportError(e))
            if e.code == quinn::TransportErrorCode::crypto(NO_APPLICATION_PROTOCOL) =>
        {
            info!("{addr}: refused, incompatible client version (no common protocol major)");
            return;
        }
        Err(e) => {
            debug!("{addr}: handshake failed: {e}");
            return;
        }
    };
    if let Err(e) = run(server, conn, addr, pending).await {
        debug!("{addr}: connection closed: {e:#}");
    }
}

/// The ALPN protocol agreed in the handshake.
fn negotiated(conn: &quinn::Connection) -> Option<Vec<u8>> {
    conn.handshake_data()?.downcast::<quinn::crypto::rustls::HandshakeData>().ok()?.protocol
}

/// One status answer per address per this interval; more are closed unanswered.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Status query (`shared::STATUS_ALPN`): one `ServerStatus` on a unidirectional
/// stream, then close. Counts only, no nicknames. Offered only with
/// `public_status = true` in server.toml.
async fn serve_status(server: &Server, conn: quinn::Connection, addr: SocketAddr) {
    let now = Instant::now();
    {
        let mut seen = server.status_seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.get(&addr.ip()).is_some_and(|t| now.duration_since(*t) < STATUS_INTERVAL) {
            conn.close(0u32.into(), b"too often");
            return;
        }
        if seen.len() > 4096 {
            seen.retain(|_, t| now.duration_since(*t) < Duration::from_secs(60));
        }
        seen.insert(addr.ip(), now);
    }
    let status = {
        let st = server.lock();
        shared::ServerStatus {
            name: st.config.name.clone(),
            version: shared::VERSION.to_string(),
            users_online: st.users.len() as u32,
            max_users: st.config.max_users,
            features: shared::feature_names(),
        }
    };
    let answer = async {
        let mut stream = conn.open_uni().await?;
        stream.write_all(&shared::wire::encode_status(&status)).await?;
        stream.finish()?;
        let _ = timeout(Duration::from_secs(2), stream.stopped()).await;
        anyhow::Ok(())
    };
    if let Err(e) = timeout(HANDSHAKE_TIMEOUT, answer).await.unwrap_or_else(|_| Err(anyhow::anyhow!("timed out"))) {
        debug!("{addr}: status: {e:#}");
    }
    conn.close(0u32.into(), b"status");
}

async fn run(
    server: Arc<Server>,
    conn: quinn::Connection,
    addr: SocketAddr,
    pending: OwnedSemaphorePermit,
) -> anyhow::Result<()> {
    let (mut send, mut recv) = timeout(HANDSHAKE_TIMEOUT, conn.accept_bi()).await??;
    let hello: ClientMsg = timeout(HANDSHAKE_TIMEOUT, read_msg_limited(&mut recv, MAX_HELLO_BYTES))
        .await??
        .context("stream closed before Hello")?;
    let ClientMsg::Hello { client_version, features: _, nickname, password, capabilities } = hello else {
        bail!("expected Hello as the first message");
    };

    let (tx, rx) = mpsc::channel(SEND_QUEUE);
    let can_view = capabilities.contains(&MediaKind::Screen);
    let key = crate::tls::client_key(&conn);
    let major = conn
        .handshake_data()
        .and_then(|d| d.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
        .and_then(|d| d.protocol)
        .and_then(|p| shared::major_from_alpn(&p));
    debug!("{addr}: client {client_version}, protocol major {major:?}");
    let hello = Hello { client_version, nickname: nickname.clone(), password, can_view, key };
    let registered = server.lock().register(hello, addr, tx, conn.clone());
    drop(pending);
    let user_id = match registered {
        Ok(id) => id,
        Err(reason) => {
            info!("{addr}: rejected '{nickname}': {reason:?}");
            if reason == RejectReason::WrongPassword {
                tokio::time::sleep(WRONG_PASSWORD_DELAY).await;
            }
            write_msg(&mut send, &ServerMsg::Rejected { reason }).await?;
            let _ = send.finish();
            let _ = timeout(Duration::from_secs(2), send.stopped()).await;
            conn.close(0u32.into(), b"rejected");
            return Ok(());
        }
    };

    let writer = tokio::spawn(writer_task(send, rx, conn.clone()));
    // Stream and datagram reads run in two separate loops: read_msg
    // must not be cancelled mid-frame, so they cannot share one select.
    let result = tokio::select! {
        r = control_loop(&server, user_id, addr, &mut recv) => r,
        r = media_loop(&server, user_id, &conn) => r,
    };
    server.lock().unregister(user_id);
    let _ = writer.await;
    result
}

/// Writes messages to the client. Ends when the user is removed
/// from the state: disconnected, kicked or banned.
async fn writer_task(mut send: quinn::SendStream, mut rx: mpsc::Receiver<ServerMsg>, conn: quinn::Connection) {
    while let Some(msg) = rx.recv().await {
        if write_msg(&mut send, &msg).await.is_err() {
            break;
        }
    }
    let _ = send.finish();
    // Give the client a chance to receive the final messages, e.g. Kicked.
    let _ = timeout(Duration::from_secs(2), send.stopped()).await;
    conn.close(0u32.into(), b"bye");
}

async fn control_loop(
    server: &Server,
    user_id: UserId,
    addr: SocketAddr,
    recv: &mut quinn::RecvStream,
) -> anyhow::Result<()> {
    let now = Instant::now();
    let mut control = TokenBucket::new(CONTROL_BURST, CONTROL_PER_SEC, now);
    let mut chat = TokenBucket::new(CHAT_BURST, CHAT_PER_SEC, now);
    let mut join = TokenBucket::new(JOIN_BURST, JOIN_PER_SEC, now);
    while let Some(msg) = read_msg::<_, ClientMsg>(recv).await? {
        let now = Instant::now();
        if !control.try_take(1.0, now) {
            warn!("{addr} (user {user_id}): control message flood, disconnecting");
            bail!("control message flood");
        }
        if matches!(msg, ClientMsg::ChatSend { .. } | ClientMsg::PrivateSend { .. }) && !chat.try_take(1.0, now) {
            debug!("{addr} (user {user_id}): chat rate limit, message dropped");
            server.lock().send_error(user_id, ErrorCode::RateLimited);
            continue;
        }
        if matches!(msg, ClientMsg::JoinChannel { .. }) && !join.try_take(1.0, now) {
            debug!("{addr} (user {user_id}): channel switch rate limit");
            server.lock().send_error(user_id, ErrorCode::RateLimited);
            continue;
        }
        let after = server.lock().handle(user_id, msg);
        if after == After::Penalty {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(())
}

async fn media_loop(server: &Server, user_id: UserId, conn: &quinn::Connection) -> anyhow::Result<()> {
    let now = Instant::now();
    let mut voice = TokenBucket::new(VOICE_BURST_BYTES, VOICE_BYTES_PER_SEC, now);
    let mut screen = TokenBucket::new(SCREEN_BURST_BYTES, SCREEN_BYTES_PER_SEC, now);
    loop {
        let data = conn.read_datagram().await?;
        if data.len() > MAX_MEDIA_DATAGRAM {
            continue;
        }
        let Some(d) = media::decode_client(&data) else { continue };
        let bucket = match d.kind {
            MediaKind::Voice => &mut voice,
            MediaKind::Screen | MediaKind::ScreenAudio => &mut screen,
        };
        if !bucket.try_take(data.len() as f64 + DATAGRAM_COST_OVERHEAD, Instant::now()) {
            continue;
        }
        let targets = match d.kind {
            MediaKind::Voice => server.lock().voice_targets(user_id),
            MediaKind::Screen | MediaKind::ScreenAudio => server.lock().screen_targets(user_id),
        };
        if targets.is_empty() {
            continue;
        }
        let out = Bytes::from(media::encode_server(d.kind, user_id, d.seq, d.payload));
        for c in targets {
            let _ = c.send_datagram(out.clone());
        }
    }
}
