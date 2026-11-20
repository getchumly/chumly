//! Visual design: color tokens for both themes (tokens.rs), fonts (fonts.rs)
//! and the egui style built from them (style.rs), plus the custom widgets
//! that egui's stock widgets cannot style per design (widgets/).
//! Everything is re-exported here: call sites use `theme::Button`, `theme::pal`.
//!
//! Change colors and sizes here, not at call sites.

use eframe::egui;

mod fonts;
mod style;
mod tokens;
mod widgets;

pub use fonts::*;
pub use tokens::*;
pub use widgets::*;

/// Installs fonts and the styles for both themes; `theme` picks the active one.
pub fn install(ctx: &egui::Context, theme: egui::Theme) {
    ctx.set_fonts(fonts::fonts());
    ctx.set_style_of(egui::Theme::Dark, style::style(&DARK, true));
    ctx.set_style_of(egui::Theme::Light, style::style(&LIGHT, false));
    ctx.set_theme(theme);
}
