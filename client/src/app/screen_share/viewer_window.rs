//! The viewer window: someone else's stream or our own ("Your screen"), with
//! the strip on top (fullscreen, viewers, stats, volume or Stop sharing) and
//! the video below.

use egui::{CornerRadius, FontFamily, FontId, Margin, Rect, Sense, Stroke, pos2, vec2};

use super::*;
use crate::icons;
use crate::theme::{self, Button, pal};

/// The video in the viewer window: an egui texture (our own preview, or
/// the CPU fallback) or YUV planes drawn by the GPU renderer.
#[derive(Clone, Copy)]
pub(super) enum Video<'a> {
    Texture(&'a egui::TextureHandle),
    Yuv(&'a YuvRenderer, egui::Vec2),
}

impl Video<'_> {
    fn size(&self) -> egui::Vec2 {
        match self {
            Video::Texture(tex) => tex.size_vec2(),
            Video::Yuv(_, size) => *size,
        }
    }
}

/// What differs between someone else's stream and our own in the viewer window.
pub(super) enum WindowKind {
    /// Someone else's stream, with its volume.
    Watch { volume: f32 },
    /// Our own share: no sound; `minimized`: the shared window is minimized.
    Own { minimized: bool },
}

pub(super) struct ViewerWindow<'a> {
    pub(super) id: egui::ViewportId,
    pub(super) builder: &'a egui::ViewportBuilder,
    /// The streamer's name, or "Your screen".
    pub(super) title: &'a str,
    /// Whose viewers the eye pill lists.
    pub(super) streamer: UserId,
    pub(super) video: Option<Video<'a>>,
    pub(super) fullscreen: Option<Instant>,
    pub(super) stats: String,
    /// Diagnostics on hovering the stats: time per frame of each stage.
    pub(super) stats_hover: Option<String>,
    pub(super) waiting_key: bool,
    pub(super) kind: WindowKind,
}

/// What the viewer window was asked to do this frame.
#[derive(Default)]
pub(super) struct WindowOut {
    pub(super) close: bool,
    pub(super) fullscreen: Option<bool>,
    pub(super) volume: Option<f32>,
    pub(super) stop_share: bool,
}

/// The viewer window: a strip with Fullscreen, LIVE and the title, the
/// viewers, the stats and, on the right, the volume (someone else's stream)
/// or the no-sound note and Stop sharing (ours); the video below.
#[allow(clippy::too_many_arguments)]
pub(super) fn viewer_window(
    ctx: &egui::Context,
    w: &ViewerWindow,
    t: &Tr,
    session: Option<&Session>,
    cfg: &ClientConfig,
    pop: &mut super::popover::HoverPopover,
    custom_frame: bool,
    actions: &mut Vec<Action>,
) -> WindowOut {
    let mut out = WindowOut::default();
    ctx.show_viewport_immediate(w.id, w.builder.clone(), |ui, _class| {
        ui.set_style(ui.ctx().style_of(egui::Theme::Dark));
        let p = &theme::DARK;
        if ui.input(|i| i.viewport().close_requested()) {
            out.close = true;
        }
        if w.fullscreen.is_none() {
            let right = if custom_frame { 0 } else { 14 };
            let strip = egui::Frame::new().fill(p.bg1).inner_margin(Margin { left: 14, right, top: 0, bottom: 0 });
            egui::Panel::top("viewer-strip").exact_size(40.0).frame(strip).show(ui, |ui| {
                window_frame::title_bar(ui, custom_frame, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        let full = Button::secondary(t.fullscreen).with_icon(icons::CORNERS_OUT).small();
                        if ui.add(full).clicked() {
                            out.fullscreen = Some(true);
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            theme::badge(ui, t.live, Color32::WHITE, p.danger);
                            ui.label(theme::text_semibold(w.title, 13.0));
                        });
                        // Who is watching; the list on hover (no watch times here).
                        if let Some(s) = session {
                            let n = s.viewers_of(w.streamer).len();
                            let pill = super::viewers::eye_pill(ui, n, false, 24.0);
                            super::viewers::popover(ui, t, s, cfg, w.streamer, false, pop, &pill, false, actions);
                        }
                        let stats = ui.label(theme::mono(&w.stats, 11.0).color(p.t3));
                        if let Some(hover) = &w.stats_hover {
                            stats.on_hover_text(hover);
                        }
                        if w.waiting_key {
                            ui.label(theme::mono(t.waiting_keyframe, 11.0).color(p.warn));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if custom_frame {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                window_frame::window_buttons(ui, t);
                                ui.add_space(8.0);
                            }
                            ui.spacing_mut().item_spacing.x = 10.0;
                            match w.kind {
                                WindowKind::Watch { volume } => volume_control(ui, t, volume, &mut out),
                                WindowKind::Own { .. } => {
                                    let stop = Button::danger(t.stop_share).with_fill_icon(icons::FILL_STOP).small();
                                    if ui.add(stop).clicked() {
                                        out.stop_share = true;
                                    }
                                    no_sound_note(ui, t);
                                }
                            }
                        });
                    });
                });
            });
        }
        let frozen = matches!(w.kind, WindowKind::Own { minimized: true });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
            let tint = if frozen { Color32::WHITE.gamma_multiply(FROZEN_DIM) } else { Color32::WHITE };
            let resp = paint_video(ui, w.video, t.waiting_video, tint);
            if resp.double_clicked() {
                out.fullscreen = Some(w.fullscreen.is_none());
            }
            if frozen {
                frozen_banner(ui, resp.rect, t.preview_minimized);
            }
            if let Some(since) = w.fullscreen {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out.fullscreen = Some(false);
                }
                fullscreen_hint(ui, resp.rect, since, t.exit_fullscreen_hint);
            }
        });
        if custom_frame && w.fullscreen.is_none() {
            window_frame::edges(ui.ctx());
        }
    });
    out
}

/// The stream volume on the right of the strip (laid out right to left).
fn volume_control(ui: &mut egui::Ui, t: &Tr, volume: f32, out: &mut WindowOut) {
    let p = &theme::DARK;
    ui.add_sized([36.0, 16.0], egui::Label::new(theme::mono(format!("{:.0}%", volume * 100.0), 11.0).color(p.t2)));
    let mut v = volume;
    if theme::slider(ui, &mut v, 0.0..=2.0, 120.0, Some(1.0)).on_hover_text(t.stream_volume).changed() {
        out.volume = Some(v);
    }
    ui.label(theme::text(icons::SPEAKER_HIGH, 16.0).color(p.t2)).on_hover_text(t.stream_volume);
}

/// "Preview: no sound, only you see this" with a muted speaker, in the
/// volume slider's place (laid out right to left); truncated first when narrow.
fn no_sound_note(ui: &mut egui::Ui, t: &Tr) {
    let p = &theme::DARK;
    let icon_w = 14.0 + 6.0;
    let mut job = egui::text::LayoutJob::simple_singleline(
        t.preview_note.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        p.t3,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((ui.available_width() - icon_w).max(0.0));
    let g = ui.painter().layout_job(job);
    if g.size().x < 24.0 {
        return;
    }
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.label(g);
    ui.label(theme::text(icons::SPEAKER_SIMPLE_SLASH, 14.0).color(p.t3));
}

/// The minimized-window warning over the large preview (section 44).
fn frozen_banner(ui: &egui::Ui, rect: Rect, text: &str) {
    let painter = ui.painter_at(rect);
    let icon =
        painter.layout_no_wrap(icons::FILL_PAUSE_CIRCLE.to_owned(), FontId::new(16.0, theme::icons_fill()), WARN_ICON);
    let label = painter.layout_no_wrap(text.to_owned(), FontId::new(13.0, FontFamily::Proportional), WARN_TEXT);
    let size = vec2(14.0 + icon.size().x + 8.0 + label.size().x + 14.0, 8.0 + label.size().y.max(icon.size().y) + 8.0);
    let pill = Rect::from_min_size(pos2(rect.center().x - size.x / 2.0, rect.top() + 16.0), size);
    painter.rect_filled(pill, CornerRadius::same(theme::R_MD), WARN_BG);
    painter.rect_stroke(pill, CornerRadius::same(theme::R_MD), Stroke::new(1.0, WARN_BORDER), egui::StrokeKind::Inside);
    let x = pill.left() + 14.0;
    painter.galley(pos2(x, pill.center().y - icon.size().y / 2.0), icon.clone(), Color32::PLACEHOLDER);
    painter.galley(pos2(x + icon.size().x + 8.0, pill.center().y - label.size().y / 2.0), label, Color32::PLACEHOLDER);
}

/// "Esc or double-click to exit fullscreen" at the top, fading out.
fn fullscreen_hint(ui: &egui::Ui, rect: Rect, since: Instant, text: &str) {
    let p = &theme::DARK;
    let shown = since.elapsed().as_secs_f32();
    let total = FULLSCREEN_HINT_TIME.as_secs_f32();
    if shown >= total {
        return;
    }
    // Full for most of the time, then fades out over the last half second.
    let alpha = ((total - shown) / 0.5).clamp(0.0, 1.0);
    let g = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        p.t1.gamma_multiply(alpha),
    );
    let pill = Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0 + 15.0), g.size() + vec2(28.0, 16.0));
    ui.painter().rect_filled(pill, CornerRadius::same(theme::R_MD), p.bg1.gamma_multiply(0.85 * alpha));
    ui.painter().rect_stroke(
        pill,
        CornerRadius::same(theme::R_MD),
        Stroke::new(1.0, p.border.gamma_multiply(alpha)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(pill.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
    ui.ctx().request_repaint();
}

/// Draws a screen share frame centered in the available space,
/// preserving aspect ratio, on a black background. `tint` dims a frozen frame.
pub(super) fn paint_video(ui: &mut egui::Ui, video: Option<Video>, waiting: &str, tint: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    match video {
        Some(video) => {
            let size = video.size();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let fitted = Rect::from_center_size(rect.center(), size * scale);
            match video {
                Video::Texture(tex) => {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    painter.image(tex.id(), fitted, uv, tint);
                }
                Video::Yuv(renderer, _) => renderer.paint(&painter, fitted, tint),
            }
        }
        None => {
            let p = pal(ui);
            let spinner = Rect::from_center_size(rect.center() - vec2(0.0, 16.0), vec2(18.0, 18.0));
            egui::Spinner::new().size(18.0).color(p.accent).paint_at(ui, spinner);
            painter.text(
                rect.center() + vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                waiting,
                FontId::new(13.0, FontFamily::Proportional),
                p.t2,
            );
        }
    }
    resp
}
