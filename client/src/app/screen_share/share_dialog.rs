//! The share dialog: what to share, in which quality, with or without sound.

use egui::{CornerRadius, FontFamily, FontId, Margin, Sense, Stroke, pos2, vec2};

use super::*;
use crate::icons;
use crate::theme::{self, Button, Dialog, pal};

/// The dialog's state while it is open.
pub(in crate::app) struct ShareDialog {
    pub(in crate::app) sources: Vec<Source>,
    pub(in crate::app) selected: Option<SourceId>,
    pub(in crate::app) quality: Quality,
    pub(in crate::app) audio: bool,
    /// Windows: prepare and encode on the graphics card.
    pub(in crate::app) hardware: bool,
}

/// One selectable source row in the share dialog.
fn source_row(ui: &mut egui::Ui, icon: &str, name: &str, meta: &str, selected: bool) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let bg = if selected {
        p.sel
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(theme::R_SM), bg);
    let c = pos2(rect.left() + 16.0, rect.center().y);
    if selected {
        ui.painter().circle_stroke(c, 5.5, Stroke::new(5.0, p.accent));
    } else {
        ui.painter().circle_stroke(c, 7.25, Stroke::new(1.5, p.t3));
    }
    let g = ui.painter().layout_no_wrap(icon.to_owned(), FontId::new(15.0, FontFamily::Proportional), p.t3);
    let mut x = rect.left() + 34.0;
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    x += 25.0;
    let mg = ui.painter().layout_no_wrap(meta.to_owned(), FontId::new(12.0, FontFamily::Monospace), p.t3);
    let meta_w = mg.size().x;
    ui.painter().galley(
        pos2(rect.right() - 8.0 - meta_w, rect.center().y - mg.size().y / 2.0),
        mg,
        Color32::PLACEHOLDER,
    );
    let max = (rect.right() - 20.0 - meta_w - x).max(20.0);
    let mut job =
        egui::text::LayoutJob::simple_singleline(name.to_owned(), FontId::new(13.0, FontFamily::Proportional), p.t1);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max);
    let ng = ui.painter().layout_job(job);
    ui.painter().galley(pos2(x, rect.center().y - ng.size().y / 2.0), ng, Color32::PLACEHOLDER);
    resp
}

impl App {
    pub(in crate::app) fn draw_share_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(mut d) = self.screen.dialog.take() else { return };
        let t = self.t();
        let p = theme::palette(ctx.global_style().visuals.dark_mode);
        let mut cancel = false;
        let mut refresh = false;
        let mut start = None;
        let mut start_button = false;
        // The footer sees the selection as of the start of the frame.
        let selected = d.selected;
        let r = Dialog::new("share-screen", t.share_title).width(540.0).closable().show(
            ctx,
            |ui| {
                ui.spacing_mut().item_spacing.y = 14.0;
                ui.horizontal(|ui| {
                    ui.label(theme::text_semibold(t.share_pick, 13.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(Button::ghost(t.refresh).with_icon(icons::ARROWS_CLOCKWISE).small()).clicked() {
                            refresh = true;
                        }
                    });
                });
                egui::Frame::new()
                    .fill(p.field)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(theme::R_MD))
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(224.0)
                            .min_scrolled_height(224.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                let screens: Vec<&Source> = d
                                    .sources
                                    .iter()
                                    .filter(|s| matches!(s.id, SourceId::Monitor(_) | SourceId::Portal))
                                    .collect();
                                let windows: Vec<&Source> =
                                    d.sources.iter().filter(|s| matches!(s.id, SourceId::Window(_))).collect();
                                for (caption, group, icon) in [
                                    (t.share_screens, screens, icons::MONITOR),
                                    (t.share_windows, windows, icons::APP_WINDOW),
                                ] {
                                    if group.is_empty() {
                                        continue;
                                    }
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0);
                                        ui.label(theme::caption(ui, caption));
                                    });
                                    for src in group {
                                        let name = match src.id {
                                            SourceId::Monitor(_) => fill(t.share_screen_n, &[("n", &src.label)]),
                                            SourceId::Window(_) => src.label.clone(),
                                            SourceId::Portal => t.share_portal.to_owned(),
                                        };
                                        let meta = if src.id == SourceId::Portal {
                                            t.share_portal_meta
                                        } else {
                                            src.meta.as_str()
                                        };
                                        let row = source_row(ui, icon, &name, meta, d.selected == Some(src.id));
                                        if row.clicked() {
                                            d.selected = Some(src.id);
                                        }
                                        if row.double_clicked() {
                                            start = Some(src.id);
                                        }
                                    }
                                }
                            });
                    });
                ui.horizontal(|ui| {
                    ui.label(theme::text(t.share_quality, 13.0).color(p.t2));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let options: Vec<(Quality, Option<&str>, &str)> =
                            Quality::ALL.iter().map(|q| (*q, None, q.label())).collect();
                        theme::segmented(ui, &mut d.quality, &options);
                    });
                });
                if crate::screen::CAN_SHARE_AUDIO {
                    // The system dialog does not say which window was picked: all sound but ours.
                    let portal = d.sources.iter().any(|s| s.id == SourceId::Portal);
                    let tip = if portal { t.share_audio_tip_portal } else { t.share_audio_tip };
                    theme::checkbox(ui, &mut d.audio, t.share_audio, Some(tip));
                } else {
                    // Not available here: shown off and disabled, with the reason.
                    let mut off = false;
                    ui.add_enabled_ui(false, |ui| {
                        theme::checkbox(ui, &mut off, t.share_audio, Some(t.share_audio_unavailable))
                    });
                }
                // Windows only: the graphics card prepares and encodes the video.
                #[cfg(windows)]
                theme::checkbox(ui, &mut d.hardware, t.share_hardware, Some(t.share_hardware_tip));
                let note = if cfg!(windows) && d.hardware { t.share_note_hw } else { t.share_note };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(theme::text(icons::CPU, 14.0).color(p.t3));
                    ui.add(egui::Label::new(theme::text(note, 12.0).color(p.t3)).wrap());
                });
            },
            |ui| {
                let go = Button::primary(t.share_start).with_icon(icons::SCREENCAST);
                start_button |= ui.add_enabled(selected.is_some(), go).clicked();
                cancel |= ui.add(Button::secondary(t.cancel)).clicked();
            },
        );
        if refresh {
            d.sources = screen::sources();
            if !d.sources.iter().any(|s| Some(s.id) == d.selected) {
                d.selected = None;
            }
        }
        if start_button {
            start = selected;
        }
        if let Some(source) = start {
            actions.push(Action::StartShare(source, d.quality, d.audio, d.hardware));
        } else if !cancel && !r.close_requested {
            self.screen.dialog = Some(d);
        }
    }
}
