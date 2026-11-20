//! Main screen while connected. egui panels take space in the order they
//! are added, so `draw_main` adds them top bar, channels, chat input, and
//! the chat takes what is left.

use super::*;

mod channels;
mod chat;
mod top_bar;

impl App {
    pub(super) fn draw_main(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        if self.session.is_none() {
            return;
        }
        self.top_bar(root, actions);
        self.channel_panel(root, actions);
        self.chat_input(root, actions);
        self.chat_area(root, actions);
    }

    /// The app icon as a texture, created on first use.
    pub(super) fn logo(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if self.logo.is_none() {
            let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/png/chumly-icon-64.png")).ok()?;
            let image =
                egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
            self.logo = Some(ctx.load_texture("app-logo", image, egui::TextureOptions::LINEAR));
        }
        self.logo.clone()
    }
}
