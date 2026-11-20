//! Design tokens: the color palette of each theme, radii, heights, and
//! colors derived from them.

use eframe::egui::{Color32, Ui};

/// Color roles. One instance per theme.
pub struct Palette {
    /// Main surface (chat).
    pub bg1: Color32,
    /// Panels and the top bar.
    pub bg2: Color32,
    /// Input fields and cards.
    pub field: Color32,
    /// Popups, menus and dialogs.
    pub pop: Color32,
    pub hover: Color32,
    pub sel: Color32,
    pub border: Color32,
    /// Border of a hovered or pressed secondary button.
    pub border_strong: Color32,
    pub t1: Color32,
    pub t2: Color32,
    pub t3: Color32,
    pub dot_idle: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    /// Text and icons drawn on the accent color.
    pub on_accent: Color32,
    /// Accent-colored text and links on a plain background (the accent itself
    /// is too light for that in the light theme).
    pub accent_text: Color32,
    pub speak: Color32,
    pub danger: Color32,
    pub danger_hover: Color32,
    pub danger_pressed: Color32,
    pub warn: Color32,
    pub gold: Color32,
    /// Chat nickname colors, picked by a stable hash of the nickname.
    pub nicks: [Color32; 6],
    /// Role colors in the order of `ROLE_COLORS` (client/src/app/roles.rs).
    pub roles: [Color32; 10],
    /// Alpha of a role chip's background (the role color with it).
    pub role_chip_alpha: u8,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const DARK: Palette = Palette {
    bg1: hex(0x111419),
    bg2: hex(0x161A20),
    field: hex(0x1D2229),
    pop: hex(0x20252D),
    hover: hex(0x232931),
    sel: hex(0x262D37),
    border: hex(0x272D36),
    border_strong: hex(0x333A44),
    t1: hex(0xE8EBF0),
    t2: hex(0xA3ABB8),
    t3: hex(0x6E7785),
    dot_idle: hex(0x3A414B),
    accent: hex(0x43DD9A),
    accent_hover: hex(0x5BE6A9),
    accent_pressed: hex(0x33C486),
    on_accent: hex(0x0A2A1B),
    accent_text: hex(0x43DD9A),
    speak: hex(0x43DD9A),
    danger: hex(0xF0524F),
    danger_hover: hex(0xF46966),
    danger_pressed: hex(0xD94341),
    warn: hex(0xF67F2F),
    gold: hex(0xFACB39),
    nicks: [hex(0x7CC4FF), hex(0xFF9C8A), hex(0xC9A6FF), hex(0x7FE0C9), hex(0xFFC870), hex(0xF49AD1)],
    roles: [
        hex(0xFACB39),
        hex(0xB4BCC8),
        hex(0xFF7A73),
        hex(0xFF9A52),
        hex(0x43DD9A),
        hex(0x3FD4D0),
        hex(0x6FB2FF),
        hex(0xB794FF),
        hex(0xF58AC6),
        hex(0x9AA3AF),
    ],
    role_chip_alpha: 0x24,
};

pub const LIGHT: Palette = Palette {
    bg1: hex(0xFFFFFF),
    bg2: hex(0xF4F5F7),
    field: hex(0xFFFFFF),
    pop: hex(0xFFFFFF),
    hover: hex(0xEAECEF),
    sel: hex(0xE3E7EC),
    border: hex(0xDCE0E5),
    border_strong: hex(0xC9CED5),
    t1: hex(0x15181D),
    t2: hex(0x4C5461),
    t3: hex(0x7A8291),
    dot_idle: hex(0xC9CED5),
    accent: hex(0x43DD9A),
    accent_hover: hex(0x3BCF8E),
    accent_pressed: hex(0x33C486),
    on_accent: hex(0x0A2A1B),
    accent_text: hex(0x0E8A5C),
    speak: hex(0x16B374),
    danger: hex(0xD93636),
    danger_hover: hex(0xCC2F2F),
    danger_pressed: hex(0xB82828),
    warn: hex(0xD9640F),
    gold: hex(0xB8890A),
    nicks: [hex(0x1F6FB8), hex(0xC2432E), hex(0x7445C7), hex(0x0E8A70), hex(0xA86A00), hex(0xB83C86)],
    roles: [
        hex(0x8F6A00),
        hex(0x6B7482),
        hex(0xC8312F),
        hex(0xB85200),
        hex(0x0E8A5C),
        hex(0x0B7F86),
        hex(0x1F66C2),
        hex(0x7140D0),
        hex(0xB8307A),
        hex(0x6E7785),
    ],
    role_chip_alpha: 0x1A,
};

/// The palette of the theme `ui` is drawn with.
pub fn pal(ui: &Ui) -> &'static Palette {
    palette(ui.visuals().dark_mode)
}

pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

// Radii.
pub const R_XS: u8 = 4;
pub const R_SM: u8 = 6;
pub const R_MD: u8 = 8;
pub const R_LG: u8 = 12;

// Heights.
pub const TOP_BAR: f32 = 44.0;
pub const BUTTON_H: f32 = 32.0;
pub const BUTTON_H_SMALL: f32 = 28.0;
pub const ROW_USER: f32 = 28.0;
pub const ROW_CHANNEL: f32 = 30.0;

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Stable color for a nickname (FNV-1a over its bytes).
pub fn nick_color(ui: &Ui, name: &str) -> Color32 {
    let mut h: u32 = 0x811c9dc5;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    let nicks = &pal(ui).nicks;
    nicks[h as usize % nicks.len()]
}
