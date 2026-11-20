//! Client networking. Runs on a background tokio runtime and talks
//! to the UI through command and event channels.
//!
//! This file has the public types and the connection manager (connect,
//! reconnect, switch servers); `connect.rs` logs in, `session.rs` runs a
//! logged-in session, `tls.rs` checks the server certificate, `link.rs`
//! measures the connection, `probe.rs` asks a server for its status.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use shared::media::MediaKind;
use shared::{ClientMsg, RejectReason, ServerMsg};
use tokio::sync::mpsc;

use crate::audio::AudioShared;
use crate::audio::mixer::Mixer;
use crate::identity::Identity;
use crate::screen::ScreenShared;

mod connect;
mod link;
mod probe;
mod session;
mod tls;

use connect::{ConnectError, connect};
pub use link::LinkStats;
pub use probe::{ServerProbe, probe};
use session::{SessionEnd, run_session};

const RECONNECT_DELAY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub address: String,
    pub nickname: String,
    pub password: String,
    pub fingerprint: Option<String>,
    /// Presented to the server as a TLS client certificate. `None` connects
    /// without one (the identity file could not be read).
    pub identity: Option<Arc<Identity>>,
}

pub enum NetCommand {
    Connect(ConnectParams),
    Disconnect,
    Send(ClientMsg),
}

#[derive(Debug, Clone)]
pub enum DisconnectReason {
    /// The user clicked "Disconnect".
    User,
    Rejected(RejectReason),
    Kicked {
        reason: String,
        banned: bool,
        /// With `banned`: the ban's duration in seconds, `None`: permanent.
        ban_secs: Option<u64>,
    },
    CertMismatch {
        saved: String,
        actual: String,
    },
    Failed(String),
    /// No protocol major in common with the server (TLS ALPN): one side is
    /// too old. Retrying is pointless.
    Incompatible,
}

#[derive(Debug, Clone)]
pub enum NetEvent {
    Connecting,
    Reconnecting {
        attempt: u32,
    },
    /// `major`: the protocol major agreed with the server.
    Connected {
        fingerprint: String,
        major: Option<u32>,
    },
    /// Connection quality, once a second while connected.
    Link(LinkStats),
    Message(ServerMsg),
    Disconnected(DisconnectReason),
}

/// Encoded voice frame from the capture thread.
pub struct VoiceOut {
    pub seq: u32,
    pub data: Vec<u8>,
}

#[derive(Clone)]
struct Events {
    tx: std::sync::mpsc::Sender<NetEvent>,
    ctx: eframe::egui::Context,
}

impl Events {
    fn emit(&self, ev: NetEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    ctx: eframe::egui::Context,
    events: std::sync::mpsc::Sender<NetEvent>,
    voice_rx: mpsc::Receiver<VoiceOut>,
    screen_rx: mpsc::Receiver<(MediaKind, Vec<u8>)>,
    mixer: Arc<Mutex<Mixer>>,
    shared: Arc<AudioShared>,
    screen: Arc<ScreenShared>,
) -> mpsc::UnboundedSender<NetCommand> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let ctx = Ctx {
        cmds: cmd_rx,
        ev: Events { tx: events, ctx },
        voice_rx,
        screen_rx,
        screen_seq: 0,
        screen_audio_seq: 0,
        mixer,
        shared,
        screen,
    };
    rt.spawn(manager(ctx));
    cmd_tx
}

struct Ctx {
    cmds: mpsc::UnboundedReceiver<NetCommand>,
    ev: Events,
    voice_rx: mpsc::Receiver<VoiceOut>,
    /// Screen share frame fragments and audio.
    screen_rx: mpsc::Receiver<(MediaKind, Vec<u8>)>,
    screen_seq: u32,
    screen_audio_seq: u32,
    mixer: Arc<Mutex<Mixer>>,
    shared: Arc<AudioShared>,
    screen: Arc<ScreenShared>,
}

/// What to do after a command received while waiting.
enum Interrupt {
    Stop,
    Switch(ConnectParams),
}

impl Ctx {
    /// Awaits `fut` while handling commands. Voice and video are
    /// dropped meanwhile: there is no connection yet.
    async fn wait<F: std::future::Future>(&mut self, fut: F) -> Result<F::Output, Interrupt> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                r = &mut fut => return Ok(r),
                c = self.cmds.recv() => match c {
                    Some(NetCommand::Connect(p)) => return Err(Interrupt::Switch(p)),
                    Some(NetCommand::Disconnect) | None => return Err(Interrupt::Stop),
                    Some(NetCommand::Send(_)) => {}
                },
                Some(_) = self.voice_rx.recv() => {}
                Some(_) = self.screen_rx.recv() => {}
            }
        }
    }
}

async fn manager(mut ctx: Ctx) {
    let mut next: Option<ConnectParams> = None;
    loop {
        let params = match next.take() {
            Some(p) => p,
            None => match ctx.wait(std::future::pending::<()>()).await {
                Err(Interrupt::Switch(p)) => p,
                Err(Interrupt::Stop) => {
                    if ctx.cmds.is_closed() {
                        return;
                    }
                    continue;
                }
                Ok(()) => unreachable!(),
            },
        };
        next = run_with_reconnect(&mut ctx, params).await;
    }
}

/// Connects and keeps the connection alive, reconnecting on loss.
/// Returns new parameters if the user picked another server.
async fn run_with_reconnect(ctx: &mut Ctx, mut params: ConnectParams) -> Option<ConnectParams> {
    let mut ever_connected = false;
    let mut attempt = 0u32;
    loop {
        ctx.ev.emit(if ever_connected { NetEvent::Reconnecting { attempt } } else { NetEvent::Connecting });
        let result = match ctx.wait(connect(&params)).await {
            Ok(r) => r,
            Err(Interrupt::Switch(p)) => return Some(p),
            Err(Interrupt::Stop) => {
                ctx.ev.emit(NetEvent::Disconnected(DisconnectReason::User));
                return None;
            }
        };
        match result {
            Ok(session) => {
                ever_connected = true;
                attempt = 0;
                // Pin the certificate for reconnects: no probe handshake next
                // time, and a different certificate is reported as a mismatch.
                if params.fingerprint.is_none() && !session.fingerprint.is_empty() {
                    params.fingerprint = Some(session.fingerprint.clone());
                }
                match run_session(ctx, session).await {
                    SessionEnd::Lost(e) => tracing::warn!("connection lost: {e}"),
                    SessionEnd::Switch(p) => return Some(p),
                    SessionEnd::Done(reason) => {
                        ctx.ev.emit(NetEvent::Disconnected(reason));
                        return None;
                    }
                }
            }
            Err(ConnectError::Fatal(reason)) => {
                ctx.ev.emit(NetEvent::Disconnected(reason));
                return None;
            }
            Err(ConnectError::Retry(e)) => {
                // Do not retry the first connection: most likely a wrong address.
                if !ever_connected {
                    ctx.ev.emit(NetEvent::Disconnected(DisconnectReason::Failed(e)));
                    return None;
                }
                tracing::warn!("reconnect failed: {e}");
            }
        }
        attempt += 1;
        ctx.ev.emit(NetEvent::Reconnecting { attempt });
        match ctx.wait(tokio::time::sleep(RECONNECT_DELAY)).await {
            Ok(()) => {}
            Err(Interrupt::Switch(p)) => return Some(p),
            Err(Interrupt::Stop) => {
                ctx.ev.emit(NetEvent::Disconnected(DisconnectReason::User));
                return None;
            }
        }
    }
}

#[cfg(test)]
mod e2e;
