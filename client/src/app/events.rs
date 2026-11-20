//! Network events (connect, disconnect, link stats), the sounds and side
//! effects of server messages, and the texts for server errors. The
//! messages themselves are handled in `server_msg.rs`.

use super::bans_ui::BannedDialog;
use super::servers::EditorField;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sound {
    Join,
    Leave,
    Message,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    Connect,
    Disconnect,
    StreamStart,
    StreamStop,
    ViewerJoin,
    ViewerLeave,
}

impl Sound {
    /// Whether the user wants this sound (its group is on).
    pub(super) fn enabled(self, g: &crate::config::SoundGroups) -> bool {
        match self {
            Sound::Connect | Sound::Disconnect => g.server,
            Sound::Join | Sound::Leave => g.channel,
            Sound::Message => g.messages,
            Sound::Mute | Sound::Unmute | Sound::Deafen | Sound::Undeafen => g.mic_sound,
            Sound::StreamStart | Sound::StreamStop => g.stream,
            Sound::ViewerJoin | Sound::ViewerLeave => g.viewers,
        }
    }
}

/// Side effects of handling server messages. Collected during
/// processing and applied afterwards to avoid borrow conflicts.
pub(super) enum Fx {
    Sound(Sound),
    Toast(ToastKind, String),
    Send(ClientMsg),
    /// The server stopped our screen share itself, e.g. on channel switch.
    ShareStopped,
    StopWatch,
}

pub(super) fn reject_text(t: &Tr, r: &RejectReason) -> String {
    match r {
        RejectReason::WrongPassword => t.reject_password.into(),
        RejectReason::InvalidNickname => t.reject_nick_invalid.into(),
        RejectReason::NicknameTaken => t.reject_nick_taken.into(),
        RejectReason::NicknameReserved => t.reject_nick_reserved.into(),
        // Shown in its own dialog (`BannedDialog`); this is a fallback.
        RejectReason::Banned { .. } => t.banned_now_title.into(),
        RejectReason::ServerFull => t.reject_full.into(),
        RejectReason::Other => t.reject_other.into(),
    }
}

pub(super) fn error_text(t: &Tr, code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Unknown => t.err_unknown,
        ErrorCode::NotAdmin => t.err_not_admin,
        ErrorCode::NoSuchUser => t.err_no_user,
        ErrorCode::NoSuchChannel => t.err_no_channel,
        ErrorCode::ChannelFull => t.err_channel_full,
        ErrorCode::MessageTooLong => t.err_too_long,
        ErrorCode::CannotTargetSelf => t.err_self,
        ErrorCode::Unsupported => t.err_unsupported,
        ErrorCode::NotStreaming => t.err_not_streaming,
        ErrorCode::InvalidChannelName => t.err_channel_name,
        ErrorCode::ChannelNameTaken => t.err_channel_taken,
        ErrorCode::LastChannel => t.err_last_channel,
        ErrorCode::IdentityRequired => t.err_identity_required,
        ErrorCode::RateLimited => t.err_rate_limited,
        ErrorCode::NoSuchRole => t.err_no_role,
        ErrorCode::InvalidRoleName => t.err_role_name,
        ErrorCode::RoleNameTaken => t.err_role_taken,
        ErrorCode::TooManyRoles => t.err_too_many_roles,
        ErrorCode::InvalidPassword => t.err_invalid_password,
    }
}

/// The viewer sound when `who`'s watching changes from `old` to `new`, as
/// heard by `me`: the streamer hears viewers of their share come and go, a
/// viewer hears their own opening and closing. None on channel moves (the
/// channel sound plays) or when the share itself ends (its sound plays).
/// `live`: whether a user is sharing now.
pub(super) fn viewer_sound(
    me: UserId,
    who: UserId,
    old: Option<UserId>,
    new: Option<UserId>,
    moved: bool,
    live: impl Fn(UserId) -> bool,
) -> Option<Sound> {
    if moved || old == new {
        return None;
    }
    if who == me {
        return match (new, old) {
            (Some(_), _) => Some(Sound::ViewerJoin),
            (None, Some(x)) if live(x) => Some(Sound::ViewerLeave),
            _ => None,
        };
    }
    if !live(me) {
        return None;
    }
    if new == Some(me) {
        Some(Sound::ViewerJoin)
    } else if old == Some(me) {
        Some(Sound::ViewerLeave)
    } else {
        None
    }
}

impl App {
    // ---------------------------------------------------------------
    // Network events
    // ---------------------------------------------------------------

    pub(super) fn process_events(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                NetEvent::Connecting => self.conn = ConnState::Connecting,
                NetEvent::Reconnecting { attempt } => {
                    // The connection dropped (not the next failed attempt).
                    if self.conn == ConnState::Connected {
                        self.play(Sound::Disconnect);
                    }
                    if let Some(ch) = self.session.as_ref().and_then(|s| s.my_channel()) {
                        self.pending_channel = Some(ch);
                    }
                    self.conn = ConnState::Reconnecting(attempt);
                }
                NetEvent::Connected { fingerprint, major } => {
                    self.conn = ConnState::Connected;
                    self.protocol_major = major;
                    // The same server under another address (IP vs domain) cannot be
                    // told by the address, but the certificate is the same.
                    let twin = self.cfg.servers.iter().enumerate().find(|(i, b)| {
                        Some(*i) != self.active
                            && b.cert_fingerprint.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(&fingerprint))
                    });
                    if let Some((_, b)) = twin {
                        let text = fill(self.t().duplicate_server_cert, &[("name", &b.name)]);
                        self.toast_kind(ToastKind::Info, text);
                    }
                    if let Some(bm) = self.active.and_then(|i| self.cfg.servers.get_mut(i))
                        && bm.cert_fingerprint.is_none()
                        && !fingerprint.is_empty()
                    {
                        bm.cert_fingerprint = Some(fingerprint);
                        self.save_config();
                    }
                }
                NetEvent::Message(m) => self.on_server_msg(m),
                NetEvent::Link(stats) => {
                    if let Some(s) = self.session.as_mut() {
                        s.link.push(stats);
                    }
                }
                NetEvent::Disconnected(reason) => self.on_disconnected(reason),
            }
        }
    }

    pub(super) fn on_disconnected(&mut self, reason: DisconnectReason) {
        let t = self.t();
        if self.session.is_some() && !matches!(self.conn, ConnState::Reconnecting(_)) {
            self.play(Sound::Disconnect);
        }
        // Back on the server list: show fresh statuses (e.g. "offline" after a failed connect).
        self.server_list.force_probe = true;
        self.conn = ConnState::Idle;
        self.session = None;
        self.pending_channel = None;
        self.target = ChatTarget::Channel;
        self.dialogs.close_session_dialogs();
        self.admin_panel = None;
        self.stop_share(false);
        self.stop_watch();
        let server = self
            .active
            .and_then(|i| self.cfg.servers.get(i))
            .map(|b| if b.name.is_empty() { b.address.clone() } else { b.name.clone() })
            .unwrap_or_default();
        self.dialogs.info = match reason {
            DisconnectReason::User => None,
            DisconnectReason::Rejected(RejectReason::Banned { reason, remaining_secs }) => {
                self.dialogs.banned = Some(BannedDialog::new(server, false, remaining_secs, reason));
                None
            }
            DisconnectReason::Rejected(r) => {
                let text = reject_text(t, &r);
                // Something the user can fix in the bookmark: after the message,
                // back to its editor with the error under that field.
                let field = match r {
                    RejectReason::WrongPassword => Some(EditorField::Password),
                    RejectReason::InvalidNickname | RejectReason::NicknameTaken | RejectReason::NicknameReserved => {
                        Some(EditorField::Nickname)
                    }
                    _ => None,
                };
                self.dialogs.then_edit = field.zip(self.active).map(|(f, i)| (i, f, text.clone()));
                Some(text)
            }
            DisconnectReason::Kicked { reason, banned: true, ban_secs } => {
                self.dialogs.banned = Some(BannedDialog::new(server, true, ban_secs, reason));
                None
            }
            DisconnectReason::Kicked { reason, banned: false, .. } => {
                let reason = if reason.is_empty() { t.no_reason.to_string() } else { reason };
                Some(fill(t.kicked, &[("reason", &reason)]))
            }
            DisconnectReason::CertMismatch { saved, actual } => {
                if let Some(bookmark) = self.active {
                    self.dialogs.cert = Some(CertPrompt { bookmark, saved, actual });
                }
                None
            }
            DisconnectReason::Failed(err) => Some(fill(t.conn_failed, &[("err", &err)])),
            DisconnectReason::Incompatible => {
                // The server's version from its status, and a newer client if
                // there is one: the dialog says which side should update.
                let server = self.active.and_then(|i| self.cfg.servers.get(i)).and_then(|b| {
                    match self.server_list.probes.get(b.address.trim()) {
                        Some((net::ServerProbe::Online { status, .. }, _)) => Some(status.version.clone()),
                        _ => None,
                    }
                });
                self.dialogs.incompatible = Some(server.unwrap_or_default());
                if self.cfg.check_updates
                    && !matches!(
                        self.updates.status,
                        crate::updates::Status::Available(_) | crate::updates::Status::Checking
                    )
                {
                    let ctx = self.ctx.clone();
                    self.updates.check(&ctx);
                }
                None
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_sounds() {
        const ME: UserId = 1;
        const BOB: UserId = 2;
        const ANN: UserId = 3;
        let both_live = |_: UserId| true;
        let nobody_live = |_: UserId| false;
        // The streamer hears viewers of their own share only.
        assert_eq!(viewer_sound(ME, BOB, None, Some(ME), false, both_live), Some(Sound::ViewerJoin));
        assert_eq!(viewer_sound(ME, BOB, Some(ME), None, false, both_live), Some(Sound::ViewerLeave));
        assert_eq!(viewer_sound(ME, BOB, None, Some(ANN), false, both_live), None);
        // My share ended: its viewers are dropped without a sound each.
        assert_eq!(viewer_sound(ME, BOB, Some(ME), None, false, nobody_live), None);
        // A viewer hears their own opening and closing...
        assert_eq!(viewer_sound(ME, ME, None, Some(BOB), false, both_live), Some(Sound::ViewerJoin));
        assert_eq!(viewer_sound(ME, ME, Some(BOB), None, false, both_live), Some(Sound::ViewerLeave));
        // ...but not when the share ended under them, or on a channel move.
        assert_eq!(viewer_sound(ME, ME, Some(BOB), None, false, nobody_live), None);
        assert_eq!(viewer_sound(ME, ME, Some(BOB), None, true, both_live), None);
        assert_eq!(viewer_sound(ME, BOB, Some(ME), None, true, both_live), None);
    }
}
