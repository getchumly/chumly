//! Connection quality and its signal bars.

use eframe::egui::{self, Color32, CornerRadius, Response, Sense, Ui, vec2};

use crate::theme::*;

/// Connection quality level, ordered from best to worst (`max` = the worse).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    Good,
    Fair,
    Poor,
}

impl Quality {
    pub fn color(self, p: &Palette) -> Color32 {
        match self {
            Quality::Good => p.speak,
            Quality::Fair => p.warn,
            Quality::Poor => p.danger,
        }
    }

    /// How many of the three signal bars are lit.
    fn bars(self) -> usize {
        match self {
            Quality::Good => 3,
            Quality::Fair => 2,
            Quality::Poor => 1,
        }
    }
}

/// Three signal bars (3 px wide, 4/7/10 px tall, 2 px apart). `None`: all
/// three unlit, for an unknown state.
pub fn signal_bars(ui: &mut Ui, quality: Option<Quality>) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(13.0, 10.0), Sense::hover());
    let lit = quality.map_or(0, Quality::bars);
    let color = quality.map_or(p.dot_idle, |q| q.color(p));
    for (i, h) in [4.0, 7.0, 10.0].into_iter().enumerate() {
        let x = rect.left() + i as f32 * 5.0;
        let bar = egui::Rect::from_min_max(egui::pos2(x, rect.bottom() - h), egui::pos2(x + 3.0, rect.bottom()));
        ui.painter().rect_filled(bar, CornerRadius::same(1), if i < lit { color } else { p.dot_idle });
    }
    resp
}
