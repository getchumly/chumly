//! The crash report banner on the start screen: Copy
//! report, Open folder, Show report, ×. See `crate::crash`.

use std::path::{Path, PathBuf};

use egui::{CornerRadius, Margin, Stroke, vec2};

use super::*;
use crate::icons;
use crate::theme::{self, Button, pal};

/// Lines of the report shown in the preview; the copy has all of it.
const PREVIEW_LINES: usize = 6;
/// How long "Copy report" shows "Copied".
const COPIED_TIME: Duration = Duration::from_secs(2);

pub(super) struct CrashBanner {
    dir: PathBuf,
    /// The report's file name: closing the banner remembers it.
    name: String,
    text: String,
    expanded: bool,
    copied_at: Option<Instant>,
}

impl CrashBanner {
    /// The newest report, unless its banner was closed before.
    pub(super) fn load(config_path: &Path, dismissed: Option<&str>) -> Option<Self> {
        let dir = crate::crash::dir_for(config_path);
        let (name, text) = crate::crash::latest(&dir)?;
        (dismissed != Some(name.as_str())).then_some(Self { dir, name, text, expanded: false, copied_at: None })
    }

    fn preview(&self, t: &Tr) -> String {
        let lines: Vec<&str> = self.text.lines().collect();
        let mut s = lines.iter().take(PREVIEW_LINES).copied().collect::<Vec<_>>().join("\n");
        if lines.len() > PREVIEW_LINES {
            s.push('\n');
            s.push_str(&fill(t.crash_more, &[("n", &(lines.len() - PREVIEW_LINES).to_string())]));
        }
        s
    }
}

impl App {
    pub(super) fn crash_banner(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.crash.as_mut() else { return };
        let t = tr(self.cfg.language);
        let p = pal(ui);
        let copied = c.copied_at.is_some_and(|at| at.elapsed() < COPIED_TIME);
        if copied {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        let mut close = false;
        egui::Frame::new()
            .fill(p.field)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(theme::R_MD))
            .inner_margin(Margin { left: 14, right: 8, top: 12, bottom: 12 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui.label(theme::text(icons::BUG, 20.0).color(p.t2));
                    let body_w = (ui.available_width() - 28.0 - 12.0).max(120.0);
                    ui.allocate_ui(vec2(body_w, 0.0), |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(body_w);
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.label(theme::text_semibold(t.crash_title, 13.0));
                            ui.add(egui::Label::new(theme::text(t.crash_sub, 12.5).color(p.t2)).wrap());
                            ui.add_space(5.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                let copy = if copied {
                                    Button::secondary(t.copied).with_icon(icons::CHECK).icon_color(p.accent)
                                } else {
                                    Button::secondary(t.crash_copy).with_icon(icons::COPY)
                                };
                                if ui.add(copy.small()).clicked() {
                                    ui.ctx().copy_text(c.text.clone());
                                    c.copied_at = Some(Instant::now());
                                }
                                if ui
                                    .add(Button::secondary(t.crash_folder).with_icon(icons::FOLDER_OPEN).small())
                                    .clicked()
                                {
                                    crate::crash::open_folder(&c.dir);
                                }
                                let (caret, label) = if c.expanded {
                                    (icons::CARET_DOWN, t.crash_hide)
                                } else {
                                    (icons::CARET_RIGHT, t.crash_show)
                                };
                                if ui.add(Button::ghost(label).with_icon(caret).small()).clicked() {
                                    c.expanded = !c.expanded;
                                }
                            });
                        });
                    });
                    close = ui.add(Button::icon(icons::X).height(28.0)).clicked();
                });
                if c.expanded {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        egui::Frame::new()
                            .fill(p.bg1)
                            .stroke(Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(theme::R_SM))
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width() - 4.0);
                                ui.add(egui::Label::new(theme::mono(c.preview(t), 11.5).color(p.t2)).wrap());
                            });
                    });
                }
            });
        if close {
            self.cfg.crash_dismissed = self.crash.take().map(|c| c.name);
            self.cfg_dirty = true;
        }
    }
}
