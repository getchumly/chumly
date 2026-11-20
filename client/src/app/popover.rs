//! `HoverPopover`: the hover / pin behavior shared by the connection
//! details and the viewer lists.

use egui::Rect;

use super::*;

/// The popover opens after hovering this long.
const POPOVER_DELAY: Duration = Duration::from_millis(300);

/// Hover / pin state of a popover (connection details, viewer lists): it
/// opens after a short hover or pinned by a click; Esc or a click outside
/// unpins.
#[derive(Default)]
pub(super) struct HoverPopover {
    hover_since: Option<Instant>,
    pinned: bool,
    /// Where the popover was drawn last frame: hovering it keeps it open.
    rect: Option<Rect>,
}

impl HoverPopover {
    /// Updates the state for the anchor's response; returns whether to
    /// show the popover this frame (then call `set_rect` with where it went).
    pub(super) fn update(&mut self, ui: &egui::Ui, resp: &egui::Response) -> bool {
        let pointer = ui.ctx().pointer_hover_pos();
        let over_popover = self.rect.zip(pointer).is_some_and(|(r, p)| r.contains(p));
        if resp.clicked() {
            self.pinned = !self.pinned;
        }
        if resp.hovered() || over_popover {
            let since = *self.hover_since.get_or_insert_with(Instant::now);
            if since.elapsed() < POPOVER_DELAY {
                ui.ctx().request_repaint_after(POPOVER_DELAY);
            }
        } else {
            self.hover_since = None;
        }
        let pressed_outside = ui.input(|i| i.pointer.any_pressed()) && !resp.hovered() && !over_popover;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) || pressed_outside {
            self.pinned = false;
        }
        // A menu opened from the popover (a viewer's user menu) keeps it.
        let menu = self.rect.is_some() && egui::Popup::is_any_open(ui.ctx());
        let show = menu || self.pinned || self.hover_since.is_some_and(|t| t.elapsed() >= POPOVER_DELAY);
        if !show {
            self.rect = None;
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        show
    }

    /// Where the popover went this frame (hovering it keeps it open).
    pub(super) fn set_rect(&mut self, rect: Rect) {
        self.rect = Some(rect);
    }
}
