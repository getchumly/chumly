//! How a server role looks: its color and icon names (from `RoleInfo`)
//! mapped to the palette and the icon font, and its shown name.
//!
//! The server stores only names, so an unknown name (from a newer client)
//! falls back to the default look here.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId};
use shared::{OWNER_ROLE, Perms, RoleId, RoleInfo, Roles};

use crate::i18n::Tr;
use crate::icons;
use crate::theme::{self, Palette};

/// Role color names in the palette's order (`Palette::roles`); the server
/// stores the name.
pub(super) const COLORS: [&str; 10] =
    ["gold", "silver", "red", "orange", "green", "teal", "blue", "purple", "pink", "gray"];

/// Icon names offered in the role editor, in the picker's order; "" is no
/// icon. The crown (`shared::OWNER_ICON`) is the owner's and never offered.
pub(super) const ICONS: [&str; 13] = [
    "",
    "star-fill",
    "star",
    "shield",
    "shield-star",
    "wrench",
    "hammer",
    "heart",
    "music-note",
    "game-controller",
    "microphone",
    "lightning",
    "rocket-launch",
];

/// Unknown names (from a newer client) are gray.
pub(super) fn color(p: &Palette, name: &str) -> Color32 {
    let i = COLORS.iter().position(|c| *c == name).unwrap_or(COLORS.len() - 1);
    p.roles[i]
}

/// Background of a role chip: the role color, mostly transparent.
pub(super) fn chip_bg(p: &Palette, color: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), p.role_chip_alpha)
}

/// The glyph of an icon name and its font family (`FILL_*` glyphs need the
/// Fill font). `None`: no icon, or an unknown one.
pub(super) fn icon(name: &str) -> Option<(&'static str, FontFamily)> {
    let fill = theme::icons_fill;
    Some(match name {
        shared::OWNER_ICON => (icons::FILL_CROWN_SIMPLE, fill()),
        "star-fill" => (icons::FILL_STAR, fill()),
        "star" => (icons::STAR, FontFamily::Proportional),
        "shield" => (icons::SHIELD, FontFamily::Proportional),
        "shield-star" => (icons::FILL_SHIELD_STAR, fill()),
        "wrench" => (icons::FILL_WRENCH, fill()),
        "hammer" => (icons::FILL_HAMMER, fill()),
        "heart" => (icons::FILL_HEART, fill()),
        "music-note" => (icons::FILL_MUSIC_NOTE, fill()),
        "game-controller" => (icons::FILL_GAME_CONTROLLER, fill()),
        "microphone" => (icons::FILL_MICROPHONE, fill()),
        "lightning" => (icons::FILL_LIGHTNING, fill()),
        "rocket-launch" => (icons::FILL_ROCKET_LAUNCH, fill()),
        _ => return None,
    })
}

/// The shown name. The default roles keep their English names on the server
/// (it does not know the viewer's language) and are translated here until
/// someone renames them.
pub(super) fn name(t: &Tr, role: &RoleInfo) -> String {
    let default = Roles::defaults();
    let untouched = default.get(role.id).is_some_and(|d| d.name == role.name);
    match role.id {
        OWNER_ROLE if untouched => t.owner.into(),
        2 if untouched => t.admin.into(),
        3 if untouched => t.moderator.into(),
        _ => role.name.clone(),
    }
}

/// Name of a role by id, "No role" for none or an unknown id.
pub(super) fn name_of(t: &Tr, roles: &Roles, role: Option<RoleId>) -> String {
    role.and_then(|id| roles.get(id)).map_or_else(|| t.role_none.to_string(), |r| name(t, r))
}

/// Color of a role by id; muted for none.
pub(super) fn color_of(p: &Palette, roles: &Roles, role: Option<RoleId>) -> Color32 {
    role.and_then(|id| roles.get(id)).map_or(p.t3, |r| color(p, &r.color))
}

/// "Moderator · kick" for the icon's tooltip.
pub(super) fn tip(t: &Tr, roles: &Roles, role: &RoleInfo) -> String {
    let perms = roles.perms(Some(role.id));
    let names: Vec<&str> =
        Perms::NAMES.iter().filter(|(p, _)| perms.contains(*p)).map(|(p, _)| perm_label(t, *p)).collect();
    let rights = if names.is_empty() { t.perm_none.to_string() } else { names.join(", ") };
    format!("{} · {rights}", name(t, role))
}

/// Icon and name of a role (or "No role", in italics) as one line of text
/// for combo boxes; `lock` adds a lock after it (a role that cannot be changed).
pub(super) fn label(t: &Tr, p: &Palette, roles: &Roles, role: Option<RoleId>, lock: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let info = role.and_then(|id| roles.get(id));
    let color = color_of(p, roles, role);
    let (glyph, family, color) = match info {
        Some(r) => icon(&r.icon).map_or(("", FontFamily::Proportional, color), |(g, f)| (g, f, color)),
        None => (icons::MINUS, FontFamily::Proportional, p.t3),
    };
    job.append(glyph, 0.0, TextFormat::simple(FontId::new(13.0, family), color));
    let mut text = TextFormat::simple(FontId::new(13.0, theme::medium()), color);
    text.italics = info.is_none();
    let gap = if glyph.is_empty() { 0.0 } else { 7.0 };
    job.append(&name_of(t, roles, role), gap, text);
    if lock {
        job.append(icons::FILL_LOCK_SIMPLE, 8.0, TextFormat::simple(FontId::new(12.0, theme::icons_fill()), p.t3));
    }
    job
}

pub(super) fn perm_label(t: &Tr, p: Perms) -> &'static str {
    match p {
        Perms::KICK => t.perm_kick,
        Perms::BAN => t.perm_ban,
        Perms::CHANNELS => t.perm_channels,
        Perms::REGISTER => t.perm_register,
        Perms::ASSIGN_ROLES => t.perm_assign_roles,
        Perms::EDIT_ROLES => t.perm_edit_roles,
        _ => "",
    }
}
