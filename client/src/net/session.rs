//! A logged-in session: server messages and media datagrams in, commands,
//! voice and the screen share out, link stats once a second.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use shared::ServerMsg;
use shared::framing::{read_msg, write_msg};
use shared::media::{self, MediaKind};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::connect::{Session, negotiated_major};
use super::link::{LinkCounters, LinkStats};
use super::{ConnectParams, Ctx, DisconnectReason, Events, NetCommand, NetEvent};
use crate::audio::lock_mixer;
use crate::audio::mixer::Mixer;
use crate::screen::ScreenShared;

pub(super) enum SessionEnd {
    Lost(String),
    Switch(ConnectParams),
    Done(DisconnectReason),
}

pub(super) async fn run_session(ctx: &mut Ctx, s: Session) -> SessionEnd {
    let Session { endpoint, conn, mut send, recv, welcome, fingerprint } = s;
    ctx.ev.emit(NetEvent::Connected { fingerprint, major: negotiated_major(&conn) });
    ctx.ev.emit(NetEvent::Message(welcome));
    ctx.shared.connected.store(true, Ordering::Relaxed);

    // The control stream and datagrams are read in separate tasks:
    // read_msg must not be cancelled mid-frame.
    let mut reader = spawn_control_reader(recv, ctx.ev.clone(), ctx.screen.clone());
    let media = spawn_media_reader(conn.clone(), ctx.mixer.clone(), ctx.screen.clone());

    let mut link_tick = tokio::time::interval(Duration::from_secs(1));
    let mut link_prev = LinkCounters::read(&conn, &ctx.mixer);
    let end = loop {
        tokio::select! {
            _ = link_tick.tick() => {
                let now = LinkCounters::read(&conn, &ctx.mixer);
                ctx.ev.emit(NetEvent::Link(LinkStats::between(&link_prev, &now, conn.rtt())));
                link_prev = now;
            }
            r = &mut reader => break r.unwrap_or_else(|e| SessionEnd::Lost(e.to_string())),
            c = ctx.cmds.recv() => match c {
                Some(NetCommand::Send(m)) => {
                    if let Err(e) = write_msg(&mut send, &m).await {
                        break SessionEnd::Lost(format!("{e:#}"));
                    }
                }
                Some(NetCommand::Connect(p)) => break SessionEnd::Switch(p),
                Some(NetCommand::Disconnect) | None => break SessionEnd::Done(DisconnectReason::User),
            },
            Some(v) = ctx.voice_rx.recv() => {
                let dg = media::encode_client(MediaKind::Voice, v.seq, &v.data);
                let _ = conn.send_datagram(Bytes::from(dg));
            }
            Some((kind, p)) = ctx.screen_rx.recv() => {
                // Video and audio have separate sequence numbers: viewers detect
                // losses from gaps in them.
                let seq = if kind == MediaKind::ScreenAudio { &mut ctx.screen_audio_seq } else { &mut ctx.screen_seq };
                let dg = media::encode_client(kind, *seq, &p);
                *seq = seq.wrapping_add(1);
                let _ = conn.send_datagram(Bytes::from(dg));
            }
        }
    };

    ctx.shared.connected.store(false, Ordering::Relaxed);
    ctx.screen.all_ended();
    reader.abort();
    media.abort();
    lock_mixer(&ctx.mixer).clear();
    conn.close(0u32.into(), b"bye");
    // Let the close packet go out so the server removes us from the list right away.
    let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    end
}

/// Reads server messages until the stream ends or we are kicked.
fn spawn_control_reader(mut recv: quinn::RecvStream, ev: Events, screen: Arc<ScreenShared>) -> JoinHandle<SessionEnd> {
    tokio::spawn(async move {
        loop {
            match read_msg::<_, ServerMsg>(&mut recv).await {
                Ok(Some(ServerMsg::Kicked { reason, banned, ban_secs })) => {
                    return SessionEnd::Done(DisconnectReason::Kicked { reason, banned, ban_secs });
                }
                Ok(Some(m)) => {
                    // The stream window learns this here, not from the UI (see `ScreenShared::ended`).
                    match &m {
                        ServerMsg::UserUpdated(u) if !u.streams.contains(&MediaKind::Screen) => {
                            screen.share_ended(u.id)
                        }
                        ServerMsg::UserLeft { user_id } => screen.share_ended(*user_id),
                        _ => {}
                    }
                    ev.emit(NetEvent::Message(m));
                }
                Ok(None) => return SessionEnd::Lost("server closed the stream".into()),
                Err(e) => return SessionEnd::Lost(format!("{e:#}")),
            }
        }
    })
}

/// Routes incoming datagrams: voice to the mixer, screen fragments to the
/// decoder, the share's audio to the mixer while we watch it.
fn spawn_media_reader(conn: quinn::Connection, mixer: Arc<Mutex<Mixer>>, screen: Arc<ScreenShared>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(data) = conn.read_datagram().await {
            match media::decode_server(&data) {
                Some(d) if d.kind == MediaKind::Voice => lock_mixer(&mixer).push_voice(d.user_id, d.seq, d.payload),
                Some(d) if d.kind == MediaKind::Screen => screen.push(d.user_id, d.payload),
                Some(d) if d.kind == MediaKind::ScreenAudio && screen.is_watching(d.user_id) => {
                    lock_mixer(&mixer).push_stream(d.user_id, d.seq, d.payload)
                }
                _ => {}
            }
        }
    })
}
