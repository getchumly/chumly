//! Fonts: the families beyond egui's defaults, their font data, and text
//! helpers matching the design's type scale.

use eframe::egui::{FontData, FontDefinitions, FontFamily, RichText, Ui};

use super::*;

// Font families beyond egui's Proportional (Onest Regular) and Monospace.
const MEDIUM: &str = "medium";
const SEMIBOLD: &str = "semibold";
const BOLD: &str = "bold";
const ICONS_FILL: &str = "icons-fill";

pub fn medium() -> FontFamily {
    FontFamily::Name(MEDIUM.into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}
pub fn bold() -> FontFamily {
    FontFamily::Name(BOLD.into())
}
/// Phosphor Fill. The regular icon set is a fallback of every text family,
/// so `icons::*` can be mixed into any string; filled icons share codepoints
/// with regular ones and need this family explicitly.
pub fn icons_fill() -> FontFamily {
    FontFamily::Name(ICONS_FILL.into())
}

/// Text helpers matching the design's type scale.
pub fn text(s: impl Into<String>, size: f32) -> RichText {
    RichText::new(s).size(size)
}
pub fn text_medium(s: impl Into<String>, size: f32) -> RichText {
    RichText::new(s).family(medium()).size(size)
}
pub fn text_semibold(s: impl Into<String>, size: f32) -> RichText {
    RichText::new(s).family(semibold()).size(size)
}
pub fn text_bold(s: impl Into<String>, size: f32) -> RichText {
    RichText::new(s).family(bold()).size(size)
}
pub fn mono(s: impl Into<String>, size: f32) -> RichText {
    RichText::new(s).family(FontFamily::Monospace).size(size)
}
/// Small caps section caption, e.g. "CHANNELS".
pub fn caption(ui: &Ui, s: &str) -> RichText {
    RichText::new(s.to_uppercase()).family(semibold()).size(11.0).color(pal(ui).t3)
}

/// Fonts bundled with the app and the family each belongs to.
pub(super) fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), FontData::from_static(bytes).into());
    };
    add("onest-regular", include_bytes!("../../assets/fonts/Onest-Regular.ttf"));
    add("onest-medium", include_bytes!("../../assets/fonts/Onest-Medium.ttf"));
    add("onest-semibold", include_bytes!("../../assets/fonts/Onest-SemiBold.ttf"));
    add("onest-bold", include_bytes!("../../assets/fonts/Onest-Bold.ttf"));
    add("jetbrains-mono", include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"));
    add("phosphor", include_bytes!("../../assets/fonts/Phosphor-Subset.ttf"));
    add("phosphor-fill", include_bytes!("../../assets/fonts/Phosphor-Fill-Subset.ttf"));

    // egui's bundled fonts stay as fallbacks for emoji and symbols.
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |main: &str| {
        let mut list = vec![main.to_owned(), "phosphor".to_owned()];
        list.extend(fallback.iter().cloned());
        list
    };
    fonts.families.insert(FontFamily::Proportional, family("onest-regular"));
    fonts.families.insert(FontFamily::Name(MEDIUM.into()), family("onest-medium"));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), family("onest-semibold"));
    fonts.families.insert(FontFamily::Name(BOLD.into()), family("onest-bold"));
    fonts.families.insert(FontFamily::Monospace, family("jetbrains-mono"));
    fonts.families.insert(FontFamily::Name(ICONS_FILL.into()), vec!["phosphor-fill".to_owned()]);
    fonts
}

/// Icon + text in one string, for stock egui widgets (menus, labels).
pub fn with_icon(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

#[cfg(test)]
mod tests {
    use eframe::egui::FontId;
    use eframe::epaint::text::{Fonts, TextOptions};

    use super::*;

    /// Not ASCII and not a Phosphor icon (private use area): a character the
    /// text fonts must have.
    fn needs_text_font(c: char) -> bool {
        !c.is_ascii() && !('\u{E000}'..='\u{F8FF}').contains(&c) && !c.is_whitespace()
    }

    /// The bundled fonts are subsets: a character outside them shows as a
    /// box. Every UI string and every string literal in the client's code must
    /// be covered, in every family the UI uses.
    #[test]
    fn every_ui_character_has_a_glyph() {
        let mut f = Fonts::new(TextOptions::default(), fonts());
        let families = [FontFamily::Proportional, FontFamily::Monospace, medium(), semibold(), bold()];
        let mut missing = Vec::new();
        let mut check = |text: &str, from: &str| {
            for c in text.chars().filter(|c| needs_text_font(*c)) {
                for family in &families {
                    if !f.has_glyph(&FontId::new(13.0, family.clone()), c) {
                        missing.push(format!("U+{:04X} '{c}' in {family:?}, from {from}", c as u32));
                    }
                }
            }
        };
        for (lang, t) in [("ru", &crate::i18n::RU), ("en", &crate::i18n::EN)] {
            for s in t.all() {
                check(s, &format!("i18n {lang}: {s}"));
            }
        }
        // String literals in the sources, comments skipped (they may quote
        // symbols that are never drawn).
        let mut files = vec![std::path::PathBuf::from("src")];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().flatten().map(|e| e.path()));
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            for (n, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                for (i, lit) in code.split('"').enumerate() {
                    if i % 2 == 1 {
                        check(lit, &format!("{}:{}", path.display(), n + 1));
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "characters without a glyph:\n{}", missing.join("\n"));
    }
}
