//! Client GUI built with egui.
//!
//! `App` holds all UI state; this file has the struct, its creation and the
//! frame loop. State that belongs to one subsystem or screen is grouped in a
//! struct next to its code: `AudioState` (audio_state.rs), `ScreenState`
//! (screen_share.rs), `ServerList` (servers.rs), `SettingsUi` (settings.rs),
//! `Dialogs` (dialogs.rs). The rest of `impl App` is split by screen and concern into
//! `app/*.rs` (see the `mod` list below).
//! Child modules see `App`'s private fields; items they share with each other
//! are `pub(super)` and reach every file through `use super::*`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, RichText};
use shared::media::MediaKind;
use shared::{
    BanInfo, ChannelId, ChannelInfo, ClientMsg, ErrorCode, PanelUser, Perms, RejectReason, RoleId, ServerMsg, UserId,
    UserInfo,
};
use tokio::sync::mpsc;

use crate::audio::mixer::Mixer;
use crate::audio::sounds::Sounds;
use crate::audio::{self, AudioEngine, AudioShared, lock_mixer};
use crate::config::{ClientConfig, Language, ServerBookmark, Theme, VoiceMode, WindowGeometry};
use crate::hotkeys::{Bindings, Combo, HotkeyAction, Hotkeys};
use crate::i18n::{Tr, fill, tr};
use crate::identity::Identity;
use crate::net::{self, ConnectParams, DisconnectReason, NetCommand, NetEvent, VoiceOut};
use crate::ptt::PttWatcher;
use crate::screen::Frame;
use crate::screen::{self, Preview, PreviewMode, Quality, ScreenShared, Sharer, Source, SourceId, Viewer};
use crate::theme::ToastKind;

struct Toast {
    kind: ToastKind,
    text: String,
    at: Instant,
}

mod actions;
mod admin_panel;
mod audio_state;
mod bans_ui;
mod crash_ui;
mod dialogs;
mod events;
mod link;
mod main_view;
mod popover;
mod role_editor;
mod roles;
mod screen_share;
mod server_msg;
mod server_tab;
mod servers;
mod session;
mod settings;
mod ui_util;
mod updates_ui;
mod user_menu;
mod viewers;
mod window_frame;
mod yuv_video;

use actions::*;
use admin_panel::*;
use audio_state::AudioState;
use dialogs::*;
use events::*;
use screen_share::*;
use servers::*;
use session::*;
use settings::{SettingsTab, SettingsUi};

const TOAST_TIME: Duration = Duration::from_secs(4);
const TOAST_ERROR_TIME: Duration = Duration::from_secs(6);
/// At most this many toasts are shown at once; older ones are dropped.
const MAX_TOASTS: usize = 3;
const AUTOSAVE_DELAY: Duration = Duration::from_secs(2);
/// 60 frames per second.
const MIN_FRAME_TIME: Duration = Duration::from_micros(16_667);
/// How long to show the hint about leaving fullscreen.
const FULLSCREEN_HINT_TIME: Duration = Duration::from_secs(3);

pub struct App {
    cfg: ClientConfig,
    cfg_path: PathBuf,
    cfg_dirty: bool,
    last_save: Instant,
    ctx: egui::Context,

    // Connection and session.
    net: mpsc::UnboundedSender<NetCommand>,
    events: std::sync::mpsc::Receiver<NetEvent>,
    conn: ConnState,
    /// Index of the bookmark we are connected (or connecting) to.
    active: Option<usize>,
    session: Option<Session>,
    /// Channel to return to after reconnecting.
    pending_channel: Option<ChannelId>,
    /// Protocol major of the current connection (from `NetEvent::Connected`).
    protocol_major: Option<u32>,
    muted: bool,
    deafened: bool,

    // Chat.
    target: ChatTarget,
    input: String,
    /// Mirrors the server's chat limit, so a message that would be dropped
    /// is not sent and its text stays in the input field.
    chat_limit: ChatLimit,

    // Subsystems and screens.
    audio: AudioState,
    screen: ScreenState,
    server_list: ServerList,
    settings: SettingsUi,
    dialogs: Dialogs,
    admin_panel: Option<AdminPanel>,
    ptt: PttWatcher,
    hotkeys: Hotkeys,
    /// The client update check and its notices.
    updates: crate::updates::Updates,
    /// The user's identity, or why it could not be loaded. Without it the
    /// client connects without a key, like older versions.
    identity: Result<Arc<Identity>, String>,
    identity_path: PathBuf,
    /// The last crash report and its banner on the start screen.
    crash: Option<crash_ui::CrashBanner>,

    // Window and frame.
    icon: Option<Arc<egui::IconData>>,
    /// App icon texture for the top bar, created on first use.
    logo: Option<egui::TextureHandle>,
    /// Connection details under the top bar's indicator.
    link_popover: popover::HoverPopover,
    toasts: Vec<Toast>,
    /// The start-up actions (auto-connect) ran.
    started: bool,
    last_frame: Option<Instant>,

    // The runtime is dropped last, after everything that uses it.
    _rt: tokio::runtime::Runtime,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg_path: PathBuf) -> Self {
        let mut cfg = ClientConfig::load(&cfg_path);
        cfg.sort_favorites_first();
        // A newer version than the one that last ran: it was just updated.
        let just_updated =
            cfg.last_version.as_deref().is_some_and(|last| shared::release::is_newer(shared::VERSION, last));
        let version_changed = cfg.last_version.as_deref() != Some(shared::VERSION);
        cfg.last_version = Some(shared::VERSION.to_string());
        crate::theme::install(&cc.egui_ctx, egui_theme(cfg.theme));
        let identity_path = Identity::path_for(&cfg_path);
        let crash = crash_ui::CrashBanner::load(&cfg_path, cfg.crash_dismissed.as_deref());
        let identity = Identity::load_or_create(&identity_path).map(Arc::new).map_err(|e| {
            tracing::error!("identity {}: {e:#}", identity_path.display());
            format!("{e:#}")
        });

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("net")
            .enable_all()
            .build()
            .expect("failed to start tokio runtime");
        let shared = Arc::new(AudioShared::new(&cfg.audio));
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        lock_mixer(&mixer).master_volume = cfg.audio.output_volume;
        let (voice_tx, voice_rx) = mpsc::channel(64);
        let (ev_tx, ev_rx) = std::sync::mpsc::channel();
        // A 1080p keyframe takes a couple hundred fragments.
        let (screen_tx, screen_rx) = mpsc::channel(1024);
        let screen = Arc::new(ScreenShared::default());
        let net = net::spawn(
            &rt,
            cc.egui_ctx.clone(),
            ev_tx,
            voice_rx,
            screen_rx,
            mixer.clone(),
            shared.clone(),
            screen.clone(),
        );
        let ptt = PttWatcher::start(shared.clone(), &cfg.audio.ptt_key);
        let hotkeys = Hotkeys::start(cc.egui_ctx.clone(), bindings(&cfg));
        spawn_repaint_watcher(cc.egui_ctx.clone(), Arc::downgrade(&mixer), shared.clone());

        Self {
            cfg,
            cfg_path,
            cfg_dirty: version_changed,
            last_save: Instant::now(),
            ctx: cc.egui_ctx.clone(),
            net,
            events: ev_rx,
            conn: ConnState::Idle,
            active: None,
            session: None,
            pending_channel: None,
            protocol_major: None,
            muted: false,
            deafened: false,
            target: ChatTarget::Channel,
            input: String::new(),
            chat_limit: ChatLimit::new(),
            audio: AudioState::new(shared, mixer, voice_tx),
            screen: ScreenState::new(screen, screen_tx),
            server_list: ServerList::new(),
            settings: SettingsUi::default(),
            dialogs: Dialogs::default(),
            admin_panel: None,
            ptt,
            hotkeys,
            updates: {
                let mut u = crate::updates::Updates::default();
                u.just_updated = just_updated;
                u
            },
            identity,
            identity_path,
            crash,
            icon: crate::window_icon().map(Arc::new),
            logo: None,
            link_popover: Default::default(),
            toasts: Vec::new(),
            started: false,
            last_frame: None,
            _rt: rt,
        }
    }

    fn t(&self) -> &'static Tr {
        tr(self.cfg.language)
    }

    fn send(&self, msg: ClientMsg) {
        let _ = self.net.send(NetCommand::Send(msg));
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast_kind(ToastKind::Info, text);
    }

    fn toast_err(&mut self, text: impl Into<String>) {
        self.toast_kind(ToastKind::Error, text);
    }

    fn toast_kind(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toasts.push(Toast { kind, text: text.into(), at: Instant::now() });
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    fn play(&mut self, sound: Sound) {
        let a = &self.cfg.audio;
        if a.event_sounds && sound.enabled(&a.sound_groups) {
            self.play_now(sound);
        }
    }

    /// Plays a sound at the notification volume, whatever the settings
    /// say ("Play" in the settings).
    fn play_now(&mut self, sound: Sound) {
        self.audio.play(sound, self.cfg.audio.notify_volume.clamp(0.0, 1.0));
    }

    fn set_hotkey(&mut self, action: HotkeyAction, combo: Option<Combo>) {
        let value = combo.map(|c| c.to_string()).unwrap_or_default();
        match action {
            HotkeyAction::ToggleMute => self.cfg.hotkeys.toggle_mute = value,
            HotkeyAction::ToggleDeafen => self.cfg.hotkeys.toggle_deafen = value,
        }
        self.hotkeys.set(bindings(&self.cfg));
        self.cfg_dirty = true;
    }

    /// Remembers window positions and sizes to restore them next time.
    fn track_windows(&mut self, ctx: &egui::Context) {
        let main = ctx.input_for(egui::ViewportId::ROOT, |i| geometry(i.viewport(), self.cfg.window));
        if let Some(g) = main
            && self.cfg.window != Some(g)
        {
            self.cfg.window = Some(g);
            self.cfg_dirty = true;
        }
        if self.screen.watching.is_some() {
            let viewer = ctx.input_for(viewer_viewport(), |i| geometry(i.viewport(), self.cfg.viewer_window));
            if let Some(g) = viewer
                && self.cfg.viewer_window != Some(g)
            {
                self.cfg.viewer_window = Some(g);
                self.cfg_dirty = true;
            }
        }
    }

    fn stop_share(&mut self, notify_server: bool) {
        if self.screen.sharer.take().is_some() && notify_server && self.session.is_some() {
            self.send(ClientMsg::StopStream { kind: MediaKind::Screen });
        }
        self.screen.announced = false;
        self.screen.dialog = None;
        // The "Your screen" window closes once it is no longer drawn.
        self.screen.own_window = None;
        self.screen.preview.set_mode(PreviewMode::Off);
        *self.screen.preview_texture.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn stop_watch(&mut self) {
        // The viewer window closes by itself once it is no longer drawn.
        if let Some(w) = self.screen.watching.take() {
            w.stop();
            lock_mixer(&self.audio.mixer).clear_stream();
            self.screen.video.clear();
            if self.session.is_some() {
                self.send(ClientMsg::WatchStream { user_id: None });
            }
        }
    }

    fn save_config(&mut self) {
        self.cfg_dirty = false;
        self.last_save = Instant::now();
        if let Err(e) = self.cfg.save(&self.cfg_path) {
            tracing::error!("saving settings: {e:#}");
            let msg = format!("{}: {e}", self.t().config_save_failed);
            self.toast_err(msg);
        }
    }

    // ---------------------------------------------------------------
    // Audio
    // ---------------------------------------------------------------

    /// Audio runs while connected or connecting, and while the settings
    /// are open (the level meter, device tests).
    fn manage_audio(&mut self) {
        let needed = self.session.is_some() || self.conn != ConnState::Idle || self.settings.open;
        self.audio.manage(needed, &self.cfg.audio, &self.ctx);
    }

    // ---------------------------------------------------------------
    // Rendering
    // ---------------------------------------------------------------

    fn status_text(&self) -> (String, Color32) {
        let t = self.t();
        match self.conn {
            ConnState::Idle => (t.disconnected.into(), Color32::GRAY),
            ConnState::Connecting => (t.status_connecting.into(), Color32::from_rgb(230, 180, 60)),
            ConnState::Reconnecting(n) => {
                (fill(t.status_reconnecting, &[("n", &n.max(1).to_string())]), Color32::from_rgb(230, 180, 60))
            }
            ConnState::Connected => (t.status_connected.into(), Color32::from_rgb(80, 200, 120)),
        }
    }
}

impl eframe::App for App {
    /// Work without rendering. eframe calls it before every frame and
    /// separately while the window is minimized: then no frames are drawn,
    /// but network events, sounds and autosave keep running.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();
        self.server_list.receive_probes();
        if !self.started {
            self.started = true;
            if self.cfg.auto_connect
                && let Some(i) = self.cfg.last_used()
            {
                self.connect(i);
            }
        }
        for action in self.hotkeys.take() {
            self.apply_action(match action {
                HotkeyAction::ToggleMute => Action::ToggleMute,
                HotkeyAction::ToggleDeafen => Action::ToggleDeafen,
            });
        }
        if let Some(combo) = self.ptt.take_combo()
            && let Some(action) = self.settings.hotkey_capture.take()
        {
            self.set_hotkey(action, Some(combo));
        }
        if let Some(key) = self.ptt.take_captured() {
            self.ptt.set_key(&key);
            self.cfg.audio.ptt_key = key;
            self.cfg_dirty = true;
        }
        self.audio.shared.metering.store(self.settings.open, Ordering::Relaxed);
        self.updates.tick(ctx, self.cfg.check_updates);
        self.manage_audio();
        self.track_windows(ctx);
        if let Some(sh) = &self.screen.sharer {
            self.screen.fps.update(sh.counters().frames.load(Ordering::Relaxed));
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if self.screen.sharer.as_ref().is_some_and(|s| s.ended()) {
            self.stop_share(true);
            self.toast(self.t().share_ended);
        }
        if self.cfg_dirty {
            if self.last_save.elapsed() > AUTOSAVE_DELAY {
                self.save_config();
            } else {
                ctx.request_repaint_after(AUTOSAVE_DELAY);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Cap at 60 FPS even on 144 or 165 Hz monitors.
        if let Some(last) = self.last_frame {
            let elapsed = last.elapsed();
            if elapsed < MIN_FRAME_TIME {
                std::thread::sleep(MIN_FRAME_TIME - elapsed);
            }
        }
        self.last_frame = Some(Instant::now());

        let ctx = &ui.ctx().clone();
        // Diagnose unnecessary repaints: RUST_LOG=client=trace.
        if tracing::enabled!(tracing::Level::TRACE) {
            tracing::trace!("frame, repaint causes: {:?}", ctx.repaint_causes());
        }

        let mut actions = Vec::new();
        self.update_preview();
        if self.session.is_some() {
            self.draw_main(ui, &mut actions);
        } else {
            self.draw_server_list(ui, &mut actions);
        }
        self.draw_viewer(ctx, &mut actions);
        self.draw_own_window(ctx, &mut actions);
        self.draw_share_dialog(ctx, &mut actions);
        self.draw_settings(ctx);
        self.draw_editor(ctx);
        self.draw_dialogs(ctx);
        self.draw_whats_new(ctx);
        self.draw_restart_confirm(ctx);
        self.draw_admin_panel(ctx);
        if !self.cfg.system_frame {
            window_frame::edges(ctx);
        }
        for a in actions {
            self.apply_action(a);
        }

        if self.cfg_dirty {
            // Without this, unsaved settings would wait for the next mouse move.
            ctx.request_repaint_after(AUTOSAVE_DELAY);
        }
        // The mic level meter changes continuously and needs a timer.
        // Speaking indicators are woken by the thread from `spawn_repaint_watcher`.
        if self.settings.open {
            ctx.request_repaint_after(Duration::from_millis(66));
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.save_config();
        if self.conn != ConnState::Idle {
            // Close the connection gracefully so the server removes us from the list right away.
            let _ = self.net.send(NetCommand::Disconnect);
            std::thread::sleep(Duration::from_millis(400));
        }
    }
}

/// Wakes the UI only when the set of speakers changes. egui repaints
/// on demand, and a constant timer would make it draw frames
/// even when everyone is silent.
fn spawn_repaint_watcher(ctx: egui::Context, mixer: std::sync::Weak<Mutex<Mixer>>, shared: Arc<AudioShared>) {
    let spawned = std::thread::Builder::new().name("repaint-watcher".into()).spawn(move || {
        let mut last: (Vec<UserId>, bool) = (Vec::new(), false);
        loop {
            std::thread::sleep(Duration::from_millis(50));
            // Window closed, App dropped together with the mixer.
            let Some(mixer) = mixer.upgrade() else { break };
            let now = (lock_mixer(&mixer).speaking_users(), shared.transmitting.load(Ordering::Relaxed));
            if now != last {
                last = now;
                ctx.request_repaint();
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!("failed to start UI repaint thread: {e}");
    }
}

fn bindings(cfg: &ClientConfig) -> Bindings {
    vec![
        (HotkeyAction::ToggleMute, Combo::parse(&cfg.hotkeys.toggle_mute)),
        (HotkeyAction::ToggleDeafen, Combo::parse(&cfg.hotkeys.toggle_deafen)),
    ]
}

/// Unix time `ts` in the system time zone, formatted with `strftime` codes.
/// Empty for a time that cannot be represented.
fn local_time(ts: i64, format: &str) -> String {
    jiff::Timestamp::from_second(ts)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime(format).to_string())
        .unwrap_or_default()
}

fn egui_theme(theme: Theme) -> egui::Theme {
    match theme {
        Theme::Dark => egui::Theme::Dark,
        Theme::Light => egui::Theme::Light,
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(egui_theme(theme));
}

/// Window geometry to save. For a maximized window only that fact is
/// stored and the size is kept, so un-maximizing restores the
/// normal size.
fn geometry(v: &egui::ViewportInfo, previous: Option<WindowGeometry>) -> Option<WindowGeometry> {
    if v.minimized == Some(true) || v.fullscreen == Some(true) {
        return None;
    }
    if v.maximized == Some(true) {
        return previous.map(|p| WindowGeometry { maximized: true, ..p });
    }
    let outer = v.outer_rect?;
    let inner = v.inner_rect?;
    Some(WindowGeometry {
        x: outer.min.x,
        y: outer.min.y,
        width: inner.width(),
        height: inner.height(),
        maximized: false,
    })
}

#[cfg(test)]
mod tests {
    use super::local_time;

    #[test]
    fn local_time_formats_and_rejects_out_of_range() {
        // Mid-1971: the year is the same in every time zone.
        assert_eq!(local_time(46_000_000, "%Y"), "1971");
        assert_eq!(local_time(46_000_000, "%H:%M").len(), 5);
        assert_eq!(local_time(i64::MAX, "%Y"), "");
    }
}
