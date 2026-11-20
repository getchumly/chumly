//! Own window frame when the OS one is off (`system_frame = false`):
//! the top bar drags the window, window buttons sit at its right end,
//! thin zones along the edges resize, and a 1 px border outlines the window.
//! Moving and resizing are handed to the OS (StartDrag / BeginResize), so
//! snapping to screen edges keeps working.

use egui::{CornerRadius, CursorIcon, Pos2, Rect, ResizeDirection, Sense, Stroke, ViewportCommand, pos2, vec2};

use super::*;
use crate::theme::{self, pal};

/// Width of the zone along the window edges that resizes it.
const RESIZE_EDGE: f32 = 5.0;
/// Width of a window button, as in Windows.
const BUTTON_W: f32 = 46.0;

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Wraps a top bar's contents. With `custom` on, empty parts of the bar
/// drag the window and a double-click maximizes or restores it; widgets
/// inside keep their own clicks (the bar's sense is registered below them).
pub(super) fn title_bar<R>(ui: &mut egui::Ui, custom: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if !custom {
        return add(ui);
    }
    let builder = egui::UiBuilder::new().sense(Sense::click_and_drag());
    let inner = ui.scope_builder(builder, |ui| {
        ui.set_min_size(ui.available_size());
        // A selectable label would take the drag instead of the bar.
        ui.style_mut().interaction.selectable_labels = false;
        add(ui)
    });
    let resp = inner.response;
    let ctx = ui.ctx();
    if resp.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    inner.inner
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Minimize, maximize/restore and close, flush with the right end of a
/// right-to-left layout. Call first in that layout.
pub(super) fn window_buttons(ui: &mut egui::Ui, t: &Tr) {
    let ctx = ui.ctx().clone();
    let max = maximized(&ctx);
    if window_button(ui, Glyph::Close, t.win_close).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let (glyph, tip) = if max { (Glyph::Restore, t.win_restore) } else { (Glyph::Maximize, t.win_maximize) };
    if window_button(ui, glyph, tip).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    if window_button(ui, Glyph::Minimize, t.win_minimize).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

fn window_button(ui: &mut egui::Ui, glyph: Glyph, tip: &str) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(BUTTON_W, ui.available_height()), Sense::click());
    let resp = resp.on_hover_text(tip);
    let (bg, fg) = match (glyph, resp.hovered(), resp.is_pointer_button_down_on()) {
        (Glyph::Close, true, down) => (if down { p.danger_pressed } else { p.danger }, Color32::WHITE),
        (_, true, true) => (p.sel, p.t1),
        (_, true, false) => (p.hover, p.t1),
        _ => (Color32::TRANSPARENT, p.t2),
    };
    ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
    // Thin line glyphs, like the system ones.
    let c = rect.center();
    let s = Stroke::new(1.0, fg);
    let painter = ui.painter();
    match glyph {
        Glyph::Minimize => {
            painter.hline((c.x - 5.0)..=(c.x + 5.0), c.y, s);
        }
        Glyph::Maximize => {
            let r = Rect::from_center_size(c, vec2(10.0, 10.0));
            painter.rect_stroke(r, CornerRadius::same(1), s, egui::StrokeKind::Inside);
        }
        Glyph::Restore => {
            let front = Rect::from_min_size(pos2(c.x - 5.0, c.y - 3.0), vec2(8.0, 8.0));
            painter.rect_stroke(front, CornerRadius::same(1), s, egui::StrokeKind::Inside);
            // The back square shows only above and to the right of the front one.
            painter.hline((c.x - 3.0)..=(c.x + 5.0), c.y - 5.0, s);
            painter.vline(c.x + 5.0, (c.y - 5.0)..=(c.y + 3.0), s);
        }
        Glyph::Close => {
            painter.line_segment([pos2(c.x - 5.0, c.y - 5.0), pos2(c.x + 5.0, c.y + 5.0)], s);
            painter.line_segment([pos2(c.x - 5.0, c.y + 5.0), pos2(c.x + 5.0, c.y - 5.0)], s);
        }
    }
    resp
}

fn edge_at(rect: Rect, pos: Pos2) -> Option<ResizeDirection> {
    let left = pos.x - rect.left() < RESIZE_EDGE;
    let right = rect.right() - pos.x < RESIZE_EDGE;
    let top = pos.y - rect.top() < RESIZE_EDGE;
    let bottom = rect.bottom() - pos.y < RESIZE_EDGE;
    // Corners get a larger zone: they are hard to hit otherwise.
    let corner = RESIZE_EDGE * 3.0;
    let near_left = pos.x - rect.left() < corner;
    let near_right = rect.right() - pos.x < corner;
    let near_top = pos.y - rect.top() < corner;
    let near_bottom = rect.bottom() - pos.y < corner;
    Some(match () {
        _ if (top && near_left) || (left && near_top) => ResizeDirection::NorthWest,
        _ if (top && near_right) || (right && near_top) => ResizeDirection::NorthEast,
        _ if (bottom && near_left) || (left && near_bottom) => ResizeDirection::SouthWest,
        _ if (bottom && near_right) || (right && near_bottom) => ResizeDirection::SouthEast,
        _ if top => ResizeDirection::North,
        _ if bottom => ResizeDirection::South,
        _ if left => ResizeDirection::West,
        _ if right => ResizeDirection::East,
        _ => return None,
    })
}

fn cursor_for(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// Resize zones along the edges and a 1 px border. Call after drawing the
/// viewport's contents. Nothing to do when maximized or fullscreen.
pub(super) fn edges(ctx: &egui::Context) {
    let (max, full) =
        ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    if max || full {
        return;
    }
    let rect = ctx.content_rect();
    let border = theme::palette(ctx.global_style().visuals.dark_mode).border;
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("window-border"))).rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    let (hover, pressed) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.primary_pressed()));
    let Some(dir) = hover.and_then(|pos| edge_at(rect, pos)) else { return };
    ctx.set_cursor_icon(cursor_for(dir));
    if pressed {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
}
