//! Settings window: a sidebar with General, Audio, Hotkeys and Identity tabs.

use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::*;
use crate::icons;
use crate::identity::CodeError;
use crate::theme::{self, Button, Kind, pal};

/// How long the Copy button shows "Copied".
const COPIED_TIME: Duration = Duration::from_secs(2);
const WINDOW_SIZE: egui::Vec2 = egui::vec2(780.0, 600.0);
const NAV_WIDTH: f32 = 180.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsTab {
    #[default]
    General,
    Audio,
    Hotkeys,
    Identity,
}

/// State of the settings window (the settings themselves are in `cfg`).
#[derive(Default)]
pub(super) struct SettingsUi {
    pub(super) open: bool,
    pub(super) tab: SettingsTab,
    /// Which hotkey is currently waiting for a combination.
    pub(super) hotkey_capture: Option<HotkeyAction>,
    /// The recovery code is shown instead of dots.
    pub(super) identity_shown: bool,
    /// Text of a recovery code being restored.
    pub(super) identity_restore: Option<String>,
    pub(super) identity_restore_error: Option<CodeError>,
    /// When the recovery code was copied, to show "Copied" for a moment.
    pub(super) identity_copied_at: Option<Instant>,
    pub(super) input_devices: Vec<String>,
    pub(super) output_devices: Vec<String>,
}

pub(super) fn lang_name(l: Language) -> &'static str {
    match l {
        Language::Ru => "Русский",
        Language::En => "English",
    }
}

fn device_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<String>, devices: &[String], t: &Tr) -> bool {
    let mut changed = false;
    let current = value.clone().unwrap_or_else(|| t.default_device.to_string());
    egui::ComboBox::from_id_salt(id).width(ui.available_width()).height(300.0).selected_text(current).show_ui(
        ui,
        |ui| {
            changed |= ui.selectable_value(value, None, t.default_device).changed();
            for d in devices {
                changed |= ui.selectable_value(value, Some(d.clone()), d).changed();
            }
        },
    );
    changed
}

/// The meter shows the same range as the threshold can take.
pub(super) const METER_MIN_DB: f32 = -70.0;
pub(super) const METER_MAX_DB: f32 = -10.0;

/// Microphone level meter. In voice activation mode it shows the threshold
/// as a white line with a knob that can be dragged, and darkens the zone
/// below it; the bar is green while the voice passes.
fn level_meter(ui: &mut egui::Ui, level: f32, threshold: Option<&mut f32>, transmitting: bool) {
    let p = pal(ui);
    let sense = if threshold.is_some() { Sense::click_and_drag() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), sense);
    let to_x = |db: f32| {
        let frac = ((db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB)).clamp(0.0, 1.0);
        rect.left() + rect.width() * frac
    };
    let mut threshold = threshold;
    if let Some(thr) = threshold.as_deref_mut()
        && (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let frac = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        *thr = (METER_MIN_DB + frac * (METER_MAX_DB - METER_MIN_DB)).round();
    }
    let thr = threshold.as_deref().copied();

    let track = Rect::from_min_max(pos2(rect.left(), rect.top() + 4.0), pos2(rect.right(), rect.bottom() - 4.0));
    let radius = CornerRadius::same(theme::R_XS);
    ui.painter().rect_filled(track, radius, p.sel);
    let passes = transmitting || thr.is_some_and(|thr| level >= thr);
    let fill = if passes { p.speak } else { p.t3 };
    let bar = Rect::from_min_max(track.min, pos2(to_x(level), track.max.y));
    ui.painter().rect_filled(bar, radius, fill);
    if let Some(thr) = thr {
        let x = to_x(thr);
        let dead = Rect::from_min_max(track.min, pos2(x, track.max.y));
        let r = CornerRadius { nw: theme::R_XS, sw: theme::R_XS, ne: 0, se: 0 };
        ui.painter().rect_filled(dead, r, Color32::from_black_alpha(90));
        ui.painter().vline(x, rect.y_range(), Stroke::new(2.0, Color32::WHITE));
        ui.painter().circle_filled(pos2(x, rect.top() + 1.0), 6.0, Color32::WHITE);
        resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    }
}

/// DB scale under the meter.
fn meter_scale(ui: &mut egui::Ui) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
    let steps = ((METER_MAX_DB - METER_MIN_DB) / 10.0) as i32;
    for i in 0..=steps {
        let db = METER_MIN_DB + i as f32 * 10.0;
        let x = rect.left() + rect.width() * i as f32 / steps as f32;
        let text = if i == steps { format!("{db:.0} dB") } else { format!("{db:.0}") };
        let text = text.replace('-', "−");
        let align = match i {
            0 => egui::Align2::LEFT_CENTER,
            i if i == steps => egui::Align2::RIGHT_CENTER,
            _ => egui::Align2::CENTER_CENTER,
        };
        ui.painter().text(pos2(x, rect.center().y), align, text, FontId::new(10.0, FontFamily::Monospace), p.t3);
    }
}

fn nav_item(ui: &mut egui::Ui, icon: &str, label: &str, selected: bool) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let bg = if selected {
        p.sel
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), bg);
    let icon_color = if selected { p.accent } else { p.t2 };
    let g = ui.painter().layout_no_wrap(icon.to_owned(), FontId::new(16.0, FontFamily::Proportional), icon_color);
    let w = g.size().x;
    ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    let family = if selected { theme::semibold() } else { FontFamily::Proportional };
    let color = if selected { p.t1 } else { p.t2 };
    let g = ui.painter().layout(label.to_owned(), FontId::new(13.0, family), color, rect.width() - w - 28.0);
    ui.painter().galley(pos2(rect.left() + 18.0 + w, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    resp
}

fn row_label(ui: &mut egui::Ui, text: &str) {
    ui.label(theme::text(text, 13.0).color(pal(ui).t2));
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(theme::text(text, 12.0).color(pal(ui).t3)).wrap());
}

fn hsep(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, pal(ui).border));
}

/// A form row without a grid: fixed-width label, content in the rest.
/// Grids size their columns only from the second frame; this does not.
fn labeled_row(ui: &mut egui::Ui, label_w: f32, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(vec2(label_w, theme::BUTTON_H), layout, |ui| {
            ui.set_min_width(label_w);
            row_label(ui, label);
        });
        ui.vertical(|ui| {
            ui.set_width(width - label_w - 14.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            content(ui);
        });
    });
}

fn percent(ui: &mut egui::Ui, v: f32) {
    ui.add_sized([52.0, 16.0], egui::Label::new(theme::mono(format!("{:.0}%", v * 100.0), 12.0).color(pal(ui).t2)));
}

/// "Event sounds": the master switch, the notification volume with "Play"
/// and the sound groups. Off keeps the rest visible but disabled, so
/// turning it back on restores the setup.
fn event_sounds(ui: &mut egui::Ui, t: &Tr, a: &mut crate::config::AudioConfig, fx: &mut SettingsFx) {
    let p = pal(ui);
    // A fixed-height row: inside the scroll area a right-to-left layout
    // would center the switch in all the height there is.
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let title =
        ui.painter().layout_no_wrap(t.event_sounds.to_owned(), egui::FontId::new(13.0, theme::semibold()), p.t1);
    ui.painter().galley(row.left_top(), title, p.t1);
    let sub = ui.painter().layout_no_wrap(
        t.event_sounds_sub.to_owned(),
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        p.t3,
    );
    ui.painter().galley(row.left_top() + vec2(0.0, 19.0), sub, p.t3);
    let mut right =
        ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)));
    theme::toggle(&mut right, &mut a.event_sounds);
    ui.add_space(4.0);
    ui.add_enabled_ui(a.event_sounds, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        let play_w = theme::button_width(ui, t.play_sound, Some(icons::SPEAKER_HIGH));
        let slider_w = (ui.available_width() - 150.0 - 52.0 - play_w - 3.0 * 14.0 - 4.0).max(80.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.add_sized([150.0, 20.0], egui::Label::new(theme::text(t.notify_volume, 13.0).color(p.t2)));
            theme::slider(ui, &mut a.notify_volume, 0.0..=1.0, slider_w, None);
            percent(ui, a.notify_volume);
            if ui.add(Button::secondary(t.play_sound).with_icon(icons::SPEAKER_HIGH).small()).clicked() {
                fx.play_sound = true;
            }
        });
        hint(ui, t.notify_volume_hint);
        ui.add_space(2.0);
        ui.label(theme::caption(ui, t.sound_groups));
        let g = &mut a.sound_groups;
        let groups: [(&mut bool, &str, &str); 6] = [
            (&mut g.server, t.sound_server, t.sound_server_desc),
            (&mut g.channel, t.sound_channel, t.sound_channel_desc),
            (&mut g.messages, t.sound_messages, t.sound_messages_desc),
            (&mut g.mic_sound, t.sound_mic, t.sound_mic_desc),
            (&mut g.stream, t.sound_stream, t.sound_stream_desc),
            (&mut g.viewers, t.sound_viewers, t.sound_viewers_desc),
        ];
        let col_w = (ui.available_width() - 16.0) / 2.0;
        let mut it = groups.into_iter();
        while let Some(first) = it.next() {
            let second = it.next();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for (on, title, desc) in [Some(first), second].into_iter().flatten() {
                    ui.allocate_ui(vec2(col_w, 0.0), |ui| {
                        ui.set_width(col_w);
                        theme::checkbox(ui, on, title, Some(desc));
                    });
                }
            });
        }
    });
}

/// Collected during drawing, applied after the window closure.
#[derive(Default)]
struct SettingsFx {
    devices_changed: bool,
    theme_changed: bool,
    frame_changed: bool,
    refresh: bool,
    begin_ptt_capture: bool,
    clear_ptt: bool,
    cancel_capture: bool,
    begin_combo: Option<HotkeyAction>,
    clear_combo: Option<HotkeyAction>,
    restore: Option<Identity>,
    copied: bool,
    /// "Play": the connect sound at the notification volume.
    play_sound: bool,
}

impl App {
    pub(super) fn draw_settings(&mut self, ctx: &egui::Context) {
        if !self.settings.open {
            return;
        }
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let mut close = false;
        let mut fx = SettingsFx::default();
        let screen = ctx.content_rect();
        let size = vec2(WINDOW_SIZE.x.min(screen.width() - 24.0), WINDOW_SIZE.y.min(screen.height() - 24.0));
        let frame = theme::window_frame(p);

        egui::Window::new("settings")
            .title_bar(false)
            .frame(frame)
            .fixed_size(size)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                if theme::window_header(ui, size.x, t.settings) {
                    close = true;
                }

                let body_h = size.y - 48.0;
                let (body, _) = ui.allocate_exact_size(vec2(size.x, body_h), Sense::hover());
                {
                    // Sidebar.
                    let nav = Rect::from_min_size(body.min, vec2(NAV_WIDTH, body_h));
                    ui.painter().vline(nav.right() - 0.5, nav.y_range(), Stroke::new(1.0, p.border));
                    let mut nav_ui = ui.new_child(egui::UiBuilder::new().max_rect(nav.shrink2(vec2(8.0, 12.0))));
                    nav_ui.spacing_mut().item_spacing.y = 2.0;
                    let tabs = [
                        (SettingsTab::General, icons::SLIDERS_HORIZONTAL, t.settings_general),
                        (SettingsTab::Audio, icons::MICROPHONE, t.audio),
                        (SettingsTab::Hotkeys, icons::KEYBOARD, t.hotkeys),
                        (SettingsTab::Identity, icons::KEY, t.identity),
                    ];
                    let update = self.update_offer().is_some();
                    for (tab, icon, label) in tabs {
                        let resp = nav_item(&mut nav_ui, icon, label, self.settings.tab == tab);
                        if update && tab == SettingsTab::General {
                            let c = pos2(resp.rect.right() - 14.0, resp.rect.center().y);
                            nav_ui.painter().circle_filled(c, 3.0, p.accent);
                        }
                        if resp.clicked() {
                            self.settings.tab = tab;
                        }
                    }

                    // Content.
                    let content = Rect::from_min_max(pos2(nav.right(), body.top()), body.max);
                    let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content));
                    egui::ScrollArea::vertical().id_salt("settings-content").auto_shrink([false, false]).show(
                        &mut content_ui,
                        |ui| {
                            egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 18, bottom: 22 }).show(
                                ui,
                                |ui| {
                                    ui.set_width(size.x - NAV_WIDTH - 48.0);
                                    ui.spacing_mut().item_spacing = vec2(10.0, 14.0);
                                    match self.settings.tab {
                                        SettingsTab::General => self.settings_general(ui, &mut fx),
                                        SettingsTab::Audio => self.settings_audio(ui, &mut fx),
                                        SettingsTab::Hotkeys => self.settings_hotkeys(ui, &mut fx),
                                        SettingsTab::Identity => self.settings_identity(ui, &mut fx),
                                    }
                                },
                            );
                        },
                    );
                }
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !self.ptt.is_capturing() && !self.ptt.is_capturing_combo()
        {
            close = true;
        }
        self.apply_settings_fx(ctx, fx, t);
        if close {
            self.settings.open = false;
            self.ptt.cancel_capture();
            self.settings.hotkey_capture = None;
            self.settings.identity_restore = None;
            self.save_config();
        }
    }

    fn apply_settings_fx(&mut self, ctx: &egui::Context, fx: SettingsFx, t: &'static Tr) {
        if let Some(id) = fx.restore {
            match id.replace_file(&self.identity_path) {
                Ok(()) => {
                    tracing::info!("identity restored from a recovery code");
                    self.identity = Ok(Arc::new(id));
                    self.settings.identity_restore = None;
                    self.toast_kind(ToastKind::Success, t.identity_restored);
                }
                Err(e) => {
                    tracing::error!("saving identity: {e:#}");
                    self.toast_err(fill(t.identity_save_failed, &[("err", &format!("{e:#}"))]));
                }
            }
        }
        self.audio.shared.apply(&self.cfg.audio);
        lock_mixer(&self.audio.mixer).master_volume = self.cfg.audio.output_volume;
        if fx.theme_changed {
            apply_theme(ctx, self.cfg.theme);
        }
        if fx.frame_changed {
            let system = self.cfg.system_frame;
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Decorations(system));
            // The viewer applies a changed description by itself.
            if let Some(w) = &mut self.screen.watching {
                w.builder = w.builder.clone().with_decorations(system);
            }
        }
        if fx.devices_changed {
            self.audio.restart = true;
        }
        if fx.refresh {
            self.settings.input_devices = audio::input_device_names();
            self.settings.output_devices = audio::output_device_names();
        }
        if fx.cancel_capture {
            self.ptt.cancel_capture();
            self.settings.hotkey_capture = None;
        }
        if fx.begin_ptt_capture {
            self.settings.hotkey_capture = None;
            self.ptt.begin_capture();
        }
        if fx.clear_ptt {
            self.cfg.audio.ptt_key.clear();
            self.ptt.set_key("");
            self.cfg_dirty = true;
        }
        if let Some(action) = fx.begin_combo {
            self.settings.hotkey_capture = Some(action);
            self.ptt.begin_combo_capture();
        }
        if let Some(action) = fx.clear_combo {
            self.set_hotkey(action, None);
        }
        if fx.play_sound {
            self.play_now(Sound::Connect);
        }
        if fx.copied {
            self.settings.identity_copied_at = Some(Instant::now());
            ctx.request_repaint_after(COPIED_TIME);
        }
    }

    fn settings_general(&mut self, ui: &mut egui::Ui, fx: &mut SettingsFx) {
        let t = self.t();
        let cfg = &mut self.cfg;
        egui::Grid::new("settings-general")
            .num_columns(2)
            .spacing([14.0, 12.0])
            .min_col_width(150.0)
            .striped(false)
            .show(ui, |ui| {
                row_label(ui, t.language);
                let langs =
                    [(Language::Ru, None, lang_name(Language::Ru)), (Language::En, None, lang_name(Language::En))];
                if theme::segmented(ui, &mut cfg.language, &langs) {
                    self.cfg_dirty = true;
                }
                ui.end_row();
                row_label(ui, t.theme);
                let themes =
                    [(Theme::Dark, Some(icons::MOON), t.theme_dark), (Theme::Light, Some(icons::SUN), t.theme_light)];
                if theme::segmented(ui, &mut cfg.theme, &themes) {
                    fx.theme_changed = true;
                    self.cfg_dirty = true;
                }
                ui.end_row();
                row_label(ui, t.system_frame);
                if theme::toggle(ui, &mut cfg.system_frame).changed() {
                    fx.frame_changed = true;
                    self.cfg_dirty = true;
                }
                ui.end_row();
                ui.label("");
                hint(ui, t.system_frame_hint);
                ui.end_row();
                row_label(ui, t.auto_connect);
                if theme::toggle(ui, &mut cfg.auto_connect).changed() {
                    self.cfg_dirty = true;
                }
                ui.end_row();
                ui.label("");
                hint(ui, t.auto_connect_hint);
                ui.end_row();
            });
        hsep(ui);
        self.settings_updates(ui);
    }

    fn settings_audio(&mut self, ui: &mut egui::Ui, fx: &mut SettingsFx) {
        let t = self.t();
        let p = pal(ui);
        let level = self.audio.shared.mic_level_db.load();
        let transmitting = self.audio.shared.transmitting.load(Ordering::Relaxed);
        let a = &mut self.cfg.audio;
        // The grids below may grow the UI past the tab; later rows keep this.
        let width = ui.available_width();

        egui::Grid::new("settings-devices")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .min_col_width(120.0)
            .striped(false)
            .show(ui, |ui| {
                row_label(ui, t.input_device);
                fx.devices_changed |= device_combo(ui, "input", &mut a.input_device, &self.settings.input_devices, t);
                ui.end_row();
                row_label(ui, t.output_device);
                fx.devices_changed |=
                    device_combo(ui, "output", &mut a.output_device, &self.settings.output_devices, t);
                ui.end_row();
                ui.label("");
                if ui.add(Button::ghost(t.refresh_devices).with_icon(icons::ARROWS_CLOCKWISE).small()).clicked() {
                    fx.refresh = true;
                }
                ui.end_row();
            });
        hsep(ui);

        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            ui.horizontal(|ui| {
                ui.label(theme::text_semibold(t.voice_mode, 13.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let modes =
                        [(VoiceMode::VoiceActivation, None, t.mode_vad), (VoiceMode::PushToTalk, None, t.mode_ptt)];
                    theme::segmented(ui, &mut a.mode, &modes);
                });
            });
            ui.horizontal(|ui| {
                match a.mode {
                    VoiceMode::VoiceActivation => {
                        ui.label(theme::text(t.vad_threshold, 12.0).color(p.t2));
                        let value = format!("{:.0} dB", a.vad_threshold_db).replace('-', "−");
                        ui.label(theme::mono(value, 12.0).color(p.t1));
                    }
                    VoiceMode::PushToTalk => {
                        let key = if a.ptt_key.is_empty() { t.ptt_not_set } else { a.ptt_key.as_str() };
                        ui.label(theme::text(fill(t.ptt_in_hotkeys, &[("key", key)]), 12.0).color(p.t2));
                    }
                }
                if transmitting {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(theme::text_medium(t.transmitting, 12.0).color(p.speak));
                        let (r, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.5, p.speak);
                    });
                }
            });
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let threshold = match a.mode {
                    VoiceMode::VoiceActivation => Some(&mut a.vad_threshold_db),
                    VoiceMode::PushToTalk => None,
                };
                level_meter(ui, level, threshold, transmitting);
                meter_scale(ui);
            });
            hint(ui, t.mic_test_hint);
        });
        hsep(ui);

        let slider_w = (ui.available_width() - 150.0 - 52.0 - 28.0).max(80.0);
        egui::Grid::new("settings-audio")
            .num_columns(3)
            .spacing([14.0, 14.0])
            .min_col_width(150.0)
            .striped(false)
            .show(ui, |ui| {
                row_label(ui, t.noise_suppression);
                theme::toggle(ui, &mut a.noise_suppression);
                ui.label("");
                ui.end_row();
                row_label(ui, t.input_gain);
                theme::slider(ui, &mut a.input_gain, 0.0..=4.0, slider_w, Some(1.0));
                percent(ui, a.input_gain);
                ui.end_row();
                row_label(ui, t.output_volume);
                theme::slider(ui, &mut a.output_volume, 0.0..=2.0, slider_w, Some(1.0));
                percent(ui, a.output_volume);
                ui.end_row();
            });
        ui.scope(|ui| {
            ui.set_max_width(width);
            hsep(ui);
            event_sounds(ui, t, a, fx);
        });
        for e in &self.audio.errors {
            theme::note(ui, theme::Note::Error, &format!("{}: {e}", t.audio_error));
        }
    }

    fn settings_hotkeys(&mut self, ui: &mut egui::Ui, fx: &mut SettingsFx) {
        let t = self.t();
        let p = pal(ui);
        let available = self.ptt.available.load(Ordering::Relaxed);
        let ptt_capturing = self.ptt.is_capturing();
        let combo_capture = if self.ptt.is_capturing_combo() { self.settings.hotkey_capture } else { None };
        let busy = ptt_capturing || combo_capture.is_some();
        hint(ui, t.hotkeys_hint);

        enum Row {
            Combo(HotkeyAction),
            Ptt,
        }
        let rows = [
            (Row::Combo(HotkeyAction::ToggleMute), t.hotkey_mute),
            (Row::Combo(HotkeyAction::ToggleDeafen), t.hotkey_deafen),
            (Row::Ptt, t.mode_ptt),
        ];
        // Label, field, Assign/Cancel, Clear.
        let field_w = (ui.available_width() - 150.0 - 100.0 - 80.0 - 30.0).max(120.0);
        egui::Grid::new("settings-hotkeys")
            .num_columns(4)
            .spacing([10.0, 10.0])
            .min_col_width(0.0)
            .striped(false)
            .show(ui, |ui| {
                for (row, label) in rows {
                    let (value, capturing, failed, prompt) = match row {
                        Row::Combo(action) => {
                            let value = match action {
                                HotkeyAction::ToggleMute => self.cfg.hotkeys.toggle_mute.clone(),
                                HotkeyAction::ToggleDeafen => self.cfg.hotkeys.toggle_deafen.clone(),
                            };
                            let failed = self.hotkeys.failed(action) && !value.is_empty();
                            (value, combo_capture == Some(action), failed, t.hotkey_press)
                        }
                        Row::Ptt => (self.cfg.audio.ptt_key.clone(), ptt_capturing, false, t.ptt_press_key),
                    };
                    let layout = egui::Layout::left_to_right(egui::Align::Center);
                    ui.allocate_ui_with_layout(vec2(150.0, theme::BUTTON_H), layout, |ui| {
                        ui.set_min_width(150.0);
                        ui.label(theme::text(label, 13.0).color(p.t2));
                    });
                    theme::hotkey_field(ui, field_w, &value, t.ptt_not_set, capturing.then_some(prompt), failed);
                    if capturing {
                        if ui.add(Button::new(Kind::Secondary, t.cancel).min_width(90.0)).clicked() {
                            fx.cancel_capture = true;
                        }
                    } else if ui.add_enabled(available && !busy, Button::secondary(t.assign).min_width(90.0)).clicked()
                    {
                        match row {
                            Row::Combo(action) => fx.begin_combo = Some(action),
                            Row::Ptt => fx.begin_ptt_capture = true,
                        }
                    }
                    if ui.add_enabled(!value.is_empty() && !capturing, Button::ghost(t.hotkey_clear)).clicked() {
                        match row {
                            Row::Combo(action) => fx.clear_combo = Some(action),
                            Row::Ptt => fx.clear_ptt = true,
                        }
                    }
                    ui.end_row();
                    if failed {
                        ui.label("");
                        let text = format!("{}  {}", icons::WARNING_CIRCLE, t.hotkey_failed);
                        ui.label(theme::text(text, 12.0).color(p.danger));
                        ui.end_row();
                    }
                }
            });
    }

    fn settings_identity(&mut self, ui: &mut egui::Ui, fx: &mut SettingsFx) {
        let t = self.t();
        let p = pal(ui);
        let code = self.identity.as_ref().ok().map(|id| id.recovery_code());
        if let Err(err) = &self.identity {
            theme::note(ui, theme::Note::Error, &fill(t.identity_error, &[("err", err)]));
        }
        ui.add(egui::Label::new(theme::text(t.identity_hint, 13.0)).wrap());
        egui::Frame::new()
            .fill(p.gold.gamma_multiply(0.08))
            .stroke(Stroke::new(1.0, p.gold.gamma_multiply(0.25)))
            .corner_radius(CornerRadius::same(theme::R_MD))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(theme::text(icons::LOCK_KEY, 15.0).color(p.gold));
                    ui.add(egui::Label::new(theme::text(t.identity_keep, 12.0)).wrap());
                });
            });

        let path = self.identity_path.display().to_string();
        let copied = self.settings.identity_copied_at.is_some_and(|at| at.elapsed() < COPIED_TIME);
        labeled_row(ui, 110.0, t.file_label, |ui| {
            ui.add_space(8.0);
            ui.add(egui::Label::new(theme::mono(&path, 11.5).color(p.t2)).wrap());
        });
        if let Some(code) = &code {
            labeled_row(ui, 110.0, t.code_label, |ui| {
                let shown = if self.settings.identity_shown {
                    code.clone()
                } else {
                    code.chars().map(|c| if c == '-' { '-' } else { '•' }).collect()
                };
                egui::Frame::new()
                    .fill(p.field)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(theme::R_SM))
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(theme::mono(shown, 12.0).color(p.t1)).wrap());
                    });
                // Toggling labels get the width of the longer one, so the
                // buttons next to them do not jump.
                let show_w = theme::button_width(ui, t.identity_show, Some(icons::EYE)).max(theme::button_width(
                    ui,
                    t.identity_hide,
                    Some(icons::EYE_SLASH),
                ));
                let copy_w = theme::button_width(ui, t.copy, Some(icons::COPY)).max(theme::button_width(
                    ui,
                    t.copied,
                    Some(icons::CHECK),
                ));
                let restore_w = theme::button_width(ui, t.identity_restore, Some(icons::ARROW_COUNTER_CLOCKWISE));
                let gap = ui.spacing().item_spacing.x;
                let one_row = show_w + copy_w + restore_w + gap * 3.0 <= ui.available_width();
                let mut open_restore = false;
                ui.horizontal(|ui| {
                    let (icon, label) = if self.settings.identity_shown {
                        (icons::EYE_SLASH, t.identity_hide)
                    } else {
                        (icons::EYE, t.identity_show)
                    };
                    if ui.add(Button::secondary(label).with_icon(icon).height(30.0).width(show_w)).clicked() {
                        self.settings.identity_shown = !self.settings.identity_shown;
                    }
                    let copy = if copied {
                        Button::secondary(t.copied).with_icon(icons::CHECK).icon_color(p.accent)
                    } else {
                        Button::secondary(t.copy).with_icon(icons::COPY)
                    };
                    if ui.add(copy.height(30.0).width(copy_w)).clicked() {
                        ui.ctx().copy_text(code.clone());
                        fx.copied = true;
                    }
                    if self.settings.identity_restore.is_none() && one_row {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = Button::ghost(t.identity_restore)
                                .with_icon(icons::ARROW_COUNTER_CLOCKWISE)
                                .height(30.0);
                            open_restore |= ui.add(b).clicked();
                        });
                    }
                });
                // Not enough room (long labels): the restore link goes on its own row.
                if self.settings.identity_restore.is_none() && !one_row {
                    let b = Button::ghost(t.identity_restore).with_icon(icons::ARROW_COUNTER_CLOCKWISE).height(30.0);
                    open_restore |= ui.add(b).clicked();
                }
                if open_restore {
                    self.settings.identity_restore = Some(String::new());
                    self.settings.identity_restore_error = None;
                }
            });
        }
        if code.is_none() && self.settings.identity_restore.is_none() {
            let b = Button::secondary(t.identity_restore).with_icon(icons::ARROW_COUNTER_CLOCKWISE);
            if ui.add(b).clicked() {
                self.settings.identity_restore = Some(String::new());
                self.settings.identity_restore_error = None;
            }
        }

        let mut cancel = false;
        if let Some(text) = &mut self.settings.identity_restore {
            hsep(ui);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.label(theme::text_semibold(t.identity_restore.trim_end_matches('…'), 13.0));
                let error = self.settings.identity_restore_error;
                let stroke = if error.is_some() { p.danger } else { p.border };
                let field = egui::Frame::new()
                    .fill(p.field)
                    .stroke(Stroke::new(1.0, stroke))
                    .corner_radius(CornerRadius::same(theme::R_SM))
                    .inner_margin(Margin::symmetric(10, 8));
                let resp = ui.add(
                    egui::TextEdit::singleline(text)
                        .frame(field)
                        .font(FontId::new(12.0, FontFamily::Monospace))
                        .desired_width(ui.available_width()),
                );
                if resp.changed() {
                    self.settings.identity_restore_error = None;
                }
                ui.horizontal(|ui| {
                    if let Some(error) = error {
                        let text = match error {
                            CodeError::Invalid => t.identity_bad_code,
                            CodeError::NewerVersion => t.identity_newer_code,
                        };
                        let msg = format!("{}  {}", icons::WARNING_CIRCLE, text);
                        ui.label(theme::text(msg, 12.0).color(p.danger));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let apply = Button::danger(t.identity_restore_apply);
                        if ui.add_enabled(!text.trim().is_empty(), apply).clicked() {
                            match Identity::from_recovery_code(text) {
                                Ok(id) => fx.restore = Some(id),
                                Err(e) => self.settings.identity_restore_error = Some(e),
                            }
                        }
                        if ui.add(Button::ghost(t.cancel)).clicked() {
                            cancel = true;
                        }
                    });
                });
                hint(ui, t.identity_restore_hint);
            });
        }
        if cancel {
            self.settings.identity_restore = None;
        }
    }
}
