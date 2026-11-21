//! User actions: what the UI asks for (`Action`) and carrying it out.

use super::*;

pub(super) enum Action {
    Connect(usize),
    EditBookmark(Option<usize>),
    DeleteBookmark(usize),
    ToggleFavorite(usize),
    /// The certificate of bookmark `.0` changed (seen by the status probe):
    /// open the certificate dialog.
    ReviewCert(usize, String),
    RefreshStatus,
    /// Drag and drop: bookmark `from` to `slot` among the others.
    MoveBookmark {
        from: usize,
        slot: usize,
    },
    Disconnect,
    OpenSettings,
    OpenAdmin,
    OpenAdminPanel,
    SetRole(UserId, Option<RoleId>),
    SetRegistered(UserId, bool),
    Join(ChannelId),
    OpenPm(UserId, String),
    ClosePm(UserId),
    Select(ChatTarget),
    SetVolume(UserId, String, f32),
    Moderate {
        user_id: UserId,
        name: String,
        ban: bool,
    },
    /// Open the channel dialog: `None` means a new channel.
    EditChannel(Option<ChannelId>),
    DeleteChannel(ChannelId, String),
    ToggleMute,
    ToggleDeafen,
    SendChat,
    OpenShare,
    StartShare(SourceId, Quality, bool),
    SetStreamVolume(String, f32),
    StopShare,
    Watch(UserId, String),
    StopWatch,
    SetFullscreen(bool),
    /// Open the "Your screen" window (our own share, large).
    OpenPreview,
    /// Close it; sharing goes on.
    ClosePreview,
    SetPreviewFullscreen(bool),
    /// Show or hide the thumbnail of our own share.
    SetPreviewShown(bool),
}

impl App {
    // ---------------------------------------------------------------
    // User actions
    // ---------------------------------------------------------------

    pub(super) fn connect(&mut self, index: usize) {
        let Some(bm) = self.cfg.servers.get(index).cloned() else { return };
        // "Updated to …" shows until the first connect (section 48).
        self.updates.just_updated = false;
        let t = self.t();
        if bm.address.trim().is_empty() || !shared::validate_nickname(&bm.nickname) {
            let error = if bm.address.trim().is_empty() {
                (EditorField::Address, t.need_address)
            } else {
                (EditorField::Nickname, t.reject_nick_invalid)
            };
            self.server_list.editor =
                Some(BookmarkEditor { index: Some(index), data: bm, error: Some((error.0, error.1.into())) });
            return;
        }
        self.active = Some(index);
        self.session = None;
        self.pending_channel = None;
        self.dialogs.info = None;
        self.dialogs.then_edit = None;
        self.conn = ConnState::Connecting;
        let _ = self.net.send(NetCommand::Connect(ConnectParams {
            address: bm.address,
            nickname: bm.nickname,
            password: bm.password,
            fingerprint: bm.cert_fingerprint,
            identity: self.identity.as_ref().ok().cloned(),
        }));
    }

    pub(super) fn set_voice_state(&mut self, muted: bool, deafened: bool) {
        self.muted = muted;
        self.deafened = deafened;
        self.audio.shared.muted.store(muted, Ordering::Relaxed);
        self.audio.shared.deafened.store(deafened, Ordering::Relaxed);
        lock_mixer(&self.audio.mixer).deafened = deafened;
        if self.session.is_some() {
            self.send(ClientMsg::SetVoiceState { muted, deafened });
        }
    }

    pub(super) fn apply_action(&mut self, action: Action) {
        let t = self.t();
        match action {
            Action::Connect(i) => self.connect(i),
            Action::EditBookmark(index) => {
                // A new bookmark starts empty: the address field shows its hint.
                let data = index.and_then(|i| self.cfg.servers.get(i).cloned()).unwrap_or_default();
                self.server_list.editor = Some(BookmarkEditor { index, data, error: None });
            }
            Action::ReviewCert(bookmark, actual) => {
                if let Some(saved) = self.cfg.servers.get(bookmark).and_then(|b| b.cert_fingerprint.clone()) {
                    self.dialogs.cert = Some(CertPrompt { bookmark, saved, actual });
                }
            }
            Action::RefreshStatus => self.server_list.force_probe = true,
            Action::ToggleFavorite(i) => {
                self.cfg.toggle_favorite(i);
                self.active = None;
                self.save_config();
            }
            Action::MoveBookmark { from, slot } => {
                self.cfg.move_bookmark(from, slot);
                self.active = None;
                self.save_config();
            }
            Action::DeleteBookmark(i) => {
                if i < self.cfg.servers.len() {
                    self.cfg.servers.remove(i);
                    self.active = None;
                    self.save_config();
                }
            }
            Action::Disconnect => {
                let _ = self.net.send(NetCommand::Disconnect);
            }
            Action::OpenSettings => {
                self.settings.open = true;
                // An update is waiting: straight to where it is.
                if self.update_offer().is_some() {
                    self.settings.tab = SettingsTab::General;
                }
                self.settings.input_devices = audio::input_device_names();
                self.settings.output_devices = audio::output_device_names();
            }
            Action::OpenAdmin => self.dialogs.admin_code = Some(String::new()),
            Action::OpenAdminPanel => {
                self.admin_panel = Some(AdminPanel {
                    tab: AdminTab::Registered,
                    users: None,
                    bans: None,
                    stale: false,
                    last_request: Instant::now(),
                    roles: Default::default(),
                    server: Default::default(),
                });
                self.send(ClientMsg::ListUsers);
                self.send(ClientMsg::ListBans);
                if self.session.as_ref().and_then(|s| s.my_role()) == Some(shared::OWNER_ROLE) {
                    self.send(ClientMsg::GetServerSettings);
                }
            }
            Action::SetRole(user_id, role) => self.send(ClientMsg::SetRole { user_id, role }),
            Action::SetRegistered(user_id, registered) => self.send(ClientMsg::SetRegistered { user_id, registered }),
            Action::Join(channel_id) => self.send(ClientMsg::JoinChannel { channel_id }),
            Action::OpenPm(id, name) => {
                if let Some(s) = self.session.as_mut() {
                    let conv =
                        s.pms.entry(id).or_insert_with(|| PmConversation { name, lines: Vec::new(), unread: false });
                    conv.unread = false;
                    self.target = ChatTarget::Private(id);
                }
            }
            Action::ClosePm(id) => {
                if let Some(s) = self.session.as_mut() {
                    s.pms.remove(&id);
                }
                if self.target == ChatTarget::Private(id) {
                    self.target = ChatTarget::Channel;
                }
            }
            Action::Select(target) => {
                self.target = target;
                if let Some(s) = self.session.as_mut() {
                    match target {
                        ChatTarget::Channel => s.channel_unread = false,
                        ChatTarget::Private(id) => {
                            if let Some(c) = s.pms.get_mut(&id) {
                                c.unread = false;
                            }
                        }
                    }
                }
            }
            Action::SetVolume(id, nick, v) => {
                lock_mixer(&self.audio.mixer).set_volume(id, v);
                if (v - 1.0).abs() < 0.01 {
                    self.cfg.user_volumes.remove(&nick);
                } else {
                    self.cfg.user_volumes.insert(nick, v);
                }
                self.cfg_dirty = true;
            }
            Action::Moderate { user_id, name, ban } => {
                self.dialogs.moderate = Some(ModDialog {
                    user_id,
                    name,
                    ban,
                    reason: String::new(),
                    duration: super::bans_ui::DEFAULT_BAN,
                });
            }
            Action::EditChannel(id) => {
                let ch = id.and_then(|id| self.session.as_ref()?.channels.iter().find(|c| c.id == id).cloned());
                self.dialogs.channel = Some(match ch {
                    Some(c) => ChannelDialog {
                        id: Some(c.id),
                        name: c.name,
                        description: c.description,
                        limited: c.max_users.is_some(),
                        max_users: c.max_users.unwrap_or(10),
                    },
                    None => ChannelDialog {
                        id: None,
                        name: String::new(),
                        description: String::new(),
                        limited: false,
                        max_users: 10,
                    },
                });
            }
            Action::DeleteChannel(id, name) => self.dialogs.channel_delete = Some((id, name)),
            Action::ToggleMute => {
                let muted = !self.muted;
                // Unmuting the mic also undeafens, like in Discord.
                let deafened = if muted { self.deafened } else { false };
                self.set_voice_state(muted, deafened);
                self.play(if muted { Sound::Mute } else { Sound::Unmute });
            }
            Action::ToggleDeafen => {
                // Deafen also blocks voice transmission. The mic flag itself
                // is left alone so the mic returns to its previous state
                // after undeafening.
                let deafened = !self.deafened;
                self.set_voice_state(self.muted, deafened);
                // Effect sounds play even while deafened so the user
                // can tell the toggle worked.
                self.play(if deafened { Sound::Deafen } else { Sound::Undeafen });
            }
            Action::SendChat => {
                let text = self.input.trim().to_string();
                if text.is_empty() {
                    return;
                }
                if !self.chat_limit.try_take() {
                    // The text stays in the field: send it again in a moment.
                    self.toast_err(t.err_rate_limited);
                    return;
                }
                if text.chars().count() > shared::MAX_MESSAGE_CHARS {
                    self.toast_err(t.err_too_long);
                    return;
                }
                let Some(s) = self.session.as_ref() else { return };
                match self.target {
                    ChatTarget::Channel => {
                        let Some(channel_id) = s.my_channel() else { return };
                        self.send(ClientMsg::ChatSend { channel_id, text });
                    }
                    ChatTarget::Private(to) => self.send(ClientMsg::PrivateSend { to, text }),
                }
                self.input.clear();
            }
            Action::OpenShare => {
                if !screen::SUPPORTED {
                    self.toast_err(t.share_unsupported);
                    return;
                }
                self.screen.dialog = Some(ShareDialog {
                    sources: screen::sources(),
                    selected: None,
                    quality: self.cfg.share.quality,
                    audio: self.cfg.share.audio,
                });
            }
            Action::StartShare(source, quality, audio) => {
                self.stop_share(false);
                if self.cfg.share.quality != quality || self.cfg.share.audio != audio {
                    self.cfg.share.quality = quality;
                    self.cfg.share.audio = audio;
                    self.cfg_dirty = true;
                }
                let preview = self.screen.preview.clone();
                match Sharer::start(source, quality, audio, self.screen.tx.clone(), preview, self.ctx.clone()) {
                    Ok(sharer) => {
                        if let Some(e) = sharer.audio_error() {
                            tracing::warn!("screen share audio: {e}");
                            self.toast_err(fill(t.share_audio_failed, &[("err", e)]));
                        }
                        self.screen.sharer = Some(sharer);
                        self.screen.fps = FpsMeter::new();
                        self.send(ClientMsg::StartStream { kind: MediaKind::Screen });
                    }
                    Err(e) => {
                        tracing::warn!("screen share failed to start: {e}");
                        self.toast_err(fill(t.share_failed, &[("err", &e)]));
                    }
                }
            }
            Action::StopShare => self.stop_share(true),
            Action::Watch(user_id, name) => {
                if !screen::SUPPORTED {
                    self.toast_err(t.share_unsupported);
                    return;
                }
                if self.screen.watching.as_ref().is_some_and(|w| w.user_id == user_id) {
                    return;
                }
                self.stop_watch();
                match Viewer::start(user_id, self.screen.shared.clone(), self.net.clone(), self.ctx.clone()) {
                    Ok(viewer) => {
                        {
                            let mut mx = lock_mixer(&self.audio.mixer);
                            mx.clear_stream();
                            mx.stream_volume = self.cfg.stream_volume(&name);
                        }
                        let mut builder = egui::ViewportBuilder::default()
                            .with_title(fill(t.watching_title, &[("name", &name)]))
                            .with_inner_size([960.0, 560.0])
                            .with_min_inner_size([320.0, 200.0])
                            .with_decorations(self.cfg.system_frame);
                        if let Some(g) = self.cfg.viewer_window.and_then(|g| g.sane()) {
                            builder = builder
                                .with_inner_size([g.width, g.height])
                                .with_position([g.x, g.y])
                                .with_maximized(g.maximized);
                        }
                        if let Some(icon) = &self.icon {
                            builder = builder.with_icon(icon.clone());
                        }
                        self.screen.watching = Some(Watching {
                            user_id,
                            name,
                            viewer,
                            texture: None,
                            fullscreen: None,
                            fps: FpsMeter::new(),
                            builder,
                        });
                        self.send(ClientMsg::WatchStream { user_id: Some(user_id) });
                    }
                    Err(e) => self.toast_err(e),
                }
            }
            Action::StopWatch => self.stop_watch(),
            Action::SetStreamVolume(nick, v) => {
                lock_mixer(&self.audio.mixer).stream_volume = v;
                if (v - 1.0).abs() < 0.01 {
                    self.cfg.stream_volumes.remove(&nick);
                } else {
                    self.cfg.stream_volumes.insert(nick, v);
                }
                self.cfg_dirty = true;
            }
            Action::SetFullscreen(on) => {
                if let Some(w) = &mut self.screen.watching {
                    w.fullscreen = on.then(Instant::now);
                    self.ctx.send_viewport_cmd_to(viewer_viewport(), egui::ViewportCommand::Fullscreen(on));
                }
            }
            Action::OpenPreview => {
                if self.screen.sharer.is_none() {
                    return;
                }
                if self.screen.own_window.is_some() {
                    self.ctx.send_viewport_cmd_to(own_viewport(), egui::ViewportCommand::Focus);
                    return;
                }
                let mut builder = egui::ViewportBuilder::default()
                    .with_title(t.preview_title)
                    .with_inner_size([960.0, 560.0])
                    .with_min_inner_size([320.0, 200.0])
                    .with_decorations(self.cfg.system_frame);
                if let Some(icon) = &self.icon {
                    builder = builder.with_icon(icon.clone());
                }
                self.screen.own_window = Some(OwnWindow { fullscreen: None, builder });
            }
            // The window closes by itself once it is no longer drawn.
            Action::ClosePreview => self.screen.own_window = None,
            Action::SetPreviewFullscreen(on) => {
                if let Some(w) = &mut self.screen.own_window {
                    w.fullscreen = on.then(Instant::now);
                    self.ctx.send_viewport_cmd_to(own_viewport(), egui::ViewportCommand::Fullscreen(on));
                }
            }
            Action::SetPreviewShown(on) => {
                self.cfg.share.preview = on;
                self.cfg_dirty = true;
            }
        }
    }
}
