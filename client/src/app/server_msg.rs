//! Server messages → session state. Handlers collect side effects (`Fx`)
//! and they are applied after the message, to avoid borrow conflicts.

use super::*;

impl App {
    pub(super) fn on_server_msg(&mut self, msg: ServerMsg) {
        let t = self.t();
        self.mark_panel_stale(&msg);
        let mut fx: Vec<Fx> = Vec::new();
        match msg {
            msg @ ServerMsg::Welcome { .. } => self.on_welcome(msg, &mut fx),
            ServerMsg::UserJoined(u) => self.on_user_joined(u, &mut fx),
            ServerMsg::UserLeft { user_id } => self.on_user_left(user_id, &mut fx),
            ServerMsg::UserUpdated(u) => self.on_user_updated(u, &mut fx),
            ServerMsg::ChannelHistory { channel_id, messages } => {
                if let Some(s) = self.session.as_mut() {
                    on_channel_history(s, t, channel_id, messages);
                }
            }
            ServerMsg::Chat(m) => {
                if let Some(s) = self.session.as_mut() {
                    if self.target != ChatTarget::Channel {
                        s.channel_unread = true;
                    }
                    let line = ChatLine::Msg { author: m.author_name, text: m.text, ts: m.timestamp };
                    s.channel_chat.entry(m.channel_id).or_default().push(line);
                }
            }
            ServerMsg::Private(pm) => {
                if let Some(s) = self.session.as_mut() {
                    on_private(s, self.target, pm, &mut fx);
                }
            }
            ServerMsg::AdminSetupResult { ok } => {
                if ok && let Some(s) = self.session.as_mut() {
                    s.admin_setup = false;
                }
                fx.push(if ok {
                    Fx::Toast(ToastKind::Admin, t.admin_ok.into())
                } else {
                    Fx::Toast(ToastKind::Error, t.admin_fail.into())
                });
            }
            ServerMsg::Notice { text } => {
                if let Some(s) = self.session.as_mut() {
                    s.system_line(format!("{}: {text}", t.notice));
                }
                fx.push(Fx::Toast(ToastKind::Announce, text));
                fx.push(Fx::Sound(Sound::Message));
            }
            ServerMsg::Error { code } => {
                if code == ErrorCode::NotStreaming {
                    fx.push(Fx::StopWatch);
                }
                fx.push(Fx::Toast(ToastKind::Error, error_text(t, code).to_string()));
            }
            ServerMsg::ChannelAdded(ch) => {
                if let Some(s) = self.session.as_mut()
                    && !s.channels.iter().any(|c| c.id == ch.id)
                {
                    s.channels.push(ch);
                }
            }
            ServerMsg::ChannelUpdated(ch) => {
                if let Some(s) = self.session.as_mut()
                    && let Some(c) = s.channels.iter_mut().find(|c| c.id == ch.id)
                {
                    *c = ch;
                }
            }
            ServerMsg::ChannelRemoved { channel_id } => self.on_channel_removed(channel_id),
            ServerMsg::ServerUsers { users } => {
                if let Some(p) = self.admin_panel.as_mut() {
                    p.users = Some(users);
                }
            }
            ServerMsg::Bans { bans } => {
                if let Some(p) = self.admin_panel.as_mut() {
                    p.bans = Some(bans);
                }
            }
            ServerMsg::ServerSettings { password_set } => {
                let t = self.t();
                if let Some(text) = self.admin_panel.as_mut().and_then(|p| p.server.on_settings(t, password_set)) {
                    self.toast_kind(ToastKind::Success, text);
                }
            }
            ServerMsg::Roles(roles) => {
                if let Some(s) = self.session.as_mut() {
                    s.roles = roles;
                }
            }
            ServerMsg::KeyframeRequest => {
                if let Some(sh) = &self.screen.sharer {
                    sh.request_keyframe();
                }
            }
            // Handled by the network module.
            ServerMsg::Rejected { .. } | ServerMsg::Kicked { .. } => {}
        }
        self.apply_fx(fx);
    }

    fn apply_fx(&mut self, fx: Vec<Fx>) {
        for f in fx {
            match f {
                Fx::Sound(s) => self.play(s),
                Fx::Toast(kind, s) => self.toast_kind(kind, s),
                Fx::Send(m) => self.send(m),
                Fx::ShareStopped => {
                    self.stop_share(false);
                    self.toast(self.t().share_ended);
                }
                Fx::StopWatch => self.stop_watch(),
            }
        }
    }

    /// The open admin panel asks for a fresh user list when `msg` changes
    /// who is online or their role.
    fn mark_panel_stale(&mut self, msg: &ServerMsg) {
        let Some(p) = self.admin_panel.as_mut() else { return };
        let changes_list = match msg {
            ServerMsg::UserJoined(_) | ServerMsg::UserLeft { .. } => true,
            ServerMsg::UserUpdated(u) => self
                .session
                .as_ref()
                .and_then(|s| s.users.get(&u.id))
                .is_some_and(|old| old.role != u.role || old.registered != u.registered),
            // The panel sorts by seniority.
            ServerMsg::Roles(_) => true,
            _ => false,
        };
        p.stale |= changes_list;
    }

    /// `msg` is a `ServerMsg::Welcome`: a new session, and what the server
    /// forgot since the last one (channel, voice state, our share) is sent again.
    fn on_welcome(&mut self, msg: ServerMsg, fx: &mut Vec<Fx>) {
        let ServerMsg::Welcome {
            your_id,
            server_name,
            motd,
            channels,
            users,
            admin_setup,
            server_version,
            features,
            roles,
        } = msg
        else {
            return;
        };
        tracing::info!("server version {}", if server_version.is_empty() { "?" } else { &server_version });
        {
            let mut mx = lock_mixer(&self.audio.mixer);
            mx.clear();
            for u in &users {
                mx.set_volume(u.id, self.cfg.user_volume(&u.nickname));
            }
        }
        if let Some(bm) = self.active.and_then(|i| self.cfg.servers.get_mut(i)) {
            bm.last_connected = Some(shared::unix_now());
            self.cfg_dirty = true;
        }
        let now = Instant::now();
        let watch_since = users.iter().filter(|u| u.watching.is_some()).map(|u| (u.id, now)).collect();
        let session = Session {
            watch_since,
            viewer_flash: None,
            viewer_sounds: HashMap::new(),
            my_id: your_id,
            server_name,
            motd,
            motd_shown: false,
            channels,
            users: users.into_iter().map(|u| (u.id, u)).collect(),
            channel_chat: HashMap::new(),
            pms: BTreeMap::new(),
            channel_unread: false,
            admin_setup,
            server_version,
            server_features: features,
            link: Default::default(),
            protocol: self.protocol_major,
            roles,
        };
        let my_ch = session.my_channel();
        if let Some(ch) = self.pending_channel.take()
            && Some(ch) != my_ch
            && session.channels.iter().any(|c| c.id == ch)
        {
            fx.push(Fx::Send(ClientMsg::JoinChannel { channel_id: ch }));
        }
        if self.muted || self.deafened {
            fx.push(Fx::Send(ClientMsg::SetVoiceState { muted: self.muted, deafened: self.deafened }));
        }
        // After a reconnect the server remembers nothing: announce the
        // share again and close the viewer.
        fx.push(Fx::StopWatch);
        self.screen.announced = false;
        if let Some(sh) = &self.screen.sharer {
            fx.push(Fx::Send(ClientMsg::StartStream { kind: MediaKind::Screen }));
            sh.request_keyframe();
        }
        self.session = Some(session);
        self.target = ChatTarget::Channel;
        fx.push(Fx::Sound(Sound::Connect));
    }

    fn on_user_joined(&mut self, u: UserInfo, fx: &mut Vec<Fx>) {
        lock_mixer(&self.audio.mixer).set_volume(u.id, self.cfg.user_volume(&u.nickname));
        let t = self.t();
        if let Some(s) = self.session.as_mut() {
            s.system_line(fill(t.user_joined_server, &[("name", &u.nickname)]));
            if s.my_channel() == Some(u.channel_id) {
                fx.push(Fx::Sound(Sound::Join));
            }
            s.users.insert(u.id, u);
        }
    }

    fn on_user_left(&mut self, user_id: UserId, fx: &mut Vec<Fx>) {
        lock_mixer(&self.audio.mixer).remove_peer(user_id);
        if self.screen.watching.as_ref().is_some_and(|w| w.user_id == user_id) {
            fx.push(Fx::StopWatch);
        }
        let t = self.t();
        if let Some(s) = self.session.as_mut()
            && let Some(u) = s.users.remove(&user_id)
        {
            s.system_line(fill(t.user_left_server, &[("name", &u.nickname)]));
            if s.my_channel() == Some(u.channel_id) {
                fx.push(Fx::Sound(Sound::Leave));
            }
        }
    }

    /// Someone's channel, voice, share or watching changed: chat lines and
    /// sounds for it, and our own share and viewer follow the server.
    fn on_user_updated(&mut self, u: UserInfo, fx: &mut Vec<Fx>) {
        let t = self.t();
        let Some(s) = self.session.as_mut() else { return };
        let my_ch_before = s.my_channel();
        let old = s.users.insert(u.id, u.clone());
        let moved = old.as_ref().is_some_and(|o| o.channel_id != u.channel_id);
        if moved {
            if u.id == s.my_id {
                self.target = ChatTarget::Channel;
                s.channel_unread = false;
            }
            channel_move(s, t, old.as_ref(), &u, my_ch_before, fx);
        }
        let old_streams = old.as_ref().is_some_and(|o| o.streams.contains(&MediaKind::Screen));
        let new_streams = u.streams.contains(&MediaKind::Screen);
        // A share in my channel starts or ends (mine too). One that ends
        // because its owner moves away gets the channel sound instead.
        if !moved && old.is_some() && old_streams != new_streams && s.my_channel() == Some(u.channel_id) {
            fx.push(Fx::Sound(if new_streams { Sound::StreamStart } else { Sound::StreamStop }));
        }
        let old_watch = old.as_ref().and_then(|o| o.watching);
        if old_watch != u.watching {
            watching_changed(s, &u, old_watch, moved, fx);
        }
        if u.id == s.my_id && self.screen.sharer.is_some() {
            if new_streams {
                self.screen.announced = true;
            } else if self.screen.announced {
                fx.push(Fx::ShareStopped);
            }
        }
        // Only someone sharing in our channel can be watched.
        if let Some(w) = &self.screen.watching {
            let still_live = s
                .users
                .get(&w.user_id)
                .is_some_and(|x| x.streams.contains(&MediaKind::Screen) && Some(x.channel_id) == s.my_channel());
            if !still_live {
                fx.push(Fx::StopWatch);
            }
        }
    }

    fn on_channel_removed(&mut self, channel_id: ChannelId) {
        let t = self.t();
        if let Some(s) = self.session.as_mut()
            && let Some(pos) = s.channels.iter().position(|c| c.id == channel_id)
        {
            let ch = s.channels.remove(pos);
            s.channel_chat.remove(&channel_id);
            s.system_line(fill(t.channel_removed, &[("name", &ch.name)]));
        }
        if self.dialogs.channel.as_ref().is_some_and(|d| d.id == Some(channel_id)) {
            self.dialogs.channel = None;
        }
        if self.dialogs.channel_delete.as_ref().is_some_and(|(id, _)| *id == channel_id) {
            self.dialogs.channel_delete = None;
        }
    }
}

/// `u` moved from `old`'s channel: we moved, or someone came to or left
/// the channel we were in.
fn channel_move(
    s: &mut Session,
    t: &Tr,
    old: Option<&UserInfo>,
    u: &UserInfo,
    my_ch_before: Option<ChannelId>,
    fx: &mut Vec<Fx>,
) {
    if u.id == s.my_id {
        fx.push(Fx::Sound(Sound::Join));
    } else if my_ch_before == Some(u.channel_id) {
        s.system_line(fill(t.user_joined_channel, &[("name", &u.nickname)]));
        fx.push(Fx::Sound(Sound::Join));
    } else if old.map(|o| o.channel_id) == my_ch_before {
        s.system_line(fill(t.user_left_channel, &[("name", &u.nickname)]));
        fx.push(Fx::Sound(Sound::Leave));
    }
}

/// `u` started, stopped or switched watching (was `old_watch`).
fn watching_changed(s: &mut Session, u: &UserInfo, old_watch: Option<UserId>, moved: bool, fx: &mut Vec<Fx>) {
    match u.watching {
        Some(_) => s.watch_since.insert(u.id, Instant::now()),
        None => s.watch_since.remove(&u.id),
    };
    let me = s.my_id;
    let sound = viewer_sound(me, u.id, old_watch, u.watching, moved, |id| s.is_live(id));
    if u.id != me && u.watching == Some(me) && sound.is_some() {
        s.viewer_flash = Some(Instant::now());
    }
    if let Some(snd) = sound
        && s.viewer_sound_ok(u.id)
    {
        fx.push(Fx::Sound(snd));
    }
}

/// The channel's recent messages on joining it, then "you joined" and,
/// the first time, the message of the day.
fn on_channel_history(s: &mut Session, t: &Tr, channel_id: ChannelId, messages: Vec<shared::ChatMessage>) {
    let mut lines: Vec<ChatLine> =
        messages.into_iter().map(|m| ChatLine::Msg { author: m.author_name, text: m.text, ts: m.timestamp }).collect();
    let now = shared::unix_now();
    lines.push(ChatLine::System { text: fill(t.you_joined, &[("channel", &s.channel_name(channel_id))]), ts: now });
    if !s.motd_shown && !s.motd.is_empty() {
        lines.push(ChatLine::System { text: s.motd.clone(), ts: now });
        s.motd_shown = true;
    }
    s.channel_chat.insert(channel_id, lines);
}

/// A private message, ours (the server's echo) or to us.
fn on_private(s: &mut Session, target: ChatTarget, pm: shared::PrivateMessage, fx: &mut Vec<Fx>) {
    let incoming = pm.from_id != s.my_id;
    let (peer, peer_name) = if incoming { (pm.from_id, pm.from_name.clone()) } else { (pm.to_id, pm.to_name.clone()) };
    let conv =
        s.pms.entry(peer).or_insert_with(|| PmConversation { name: peer_name, lines: Vec::new(), unread: false });
    conv.lines.push(ChatLine::Msg { author: pm.from_name, text: pm.text, ts: pm.timestamp });
    if incoming {
        if target != ChatTarget::Private(peer) {
            conv.unread = true;
        }
        fx.push(Fx::Sound(Sound::Message));
    }
}
