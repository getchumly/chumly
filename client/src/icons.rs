//! Phosphor icon glyphs (Phosphor Icons 2.1.1, MIT).
//!
//! The bundled fonts are subsets: only the glyphs listed here exist.
//! To add an icon, add its name to tools/build_fonts.py, rebuild the
//! fonts and add the constant. Codepoints come from Phosphor's CSS.
//!
//! Regular icons work in any text (the font is a fallback of every family).
//! `FILL_*` glyphs share codepoints with regular ones: draw them with
//! `theme::icons_fill`, which selects the Fill font.
#![allow(dead_code)]

pub const APP_WINDOW: &str = "\u{E5DA}";
pub const ARROWS_CLOCKWISE: &str = "\u{E094}";
pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{E038}";
pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";
pub const AT: &str = "\u{E0AC}";
pub const BUG: &str = "\u{E5F4}";
pub const CALENDAR_BLANK: &str = "\u{E10A}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const CARET_RIGHT: &str = "\u{E13A}";
pub const CARET_UP: &str = "\u{E13C}";
pub const CHAT_CIRCLE: &str = "\u{E168}";
pub const CHECK: &str = "\u{E182}";
pub const CHECK_CIRCLE: &str = "\u{E184}";
pub const CIRCLE_DASHED: &str = "\u{E602}";
pub const CIRCLE_NOTCH: &str = "\u{EB44}";
pub const CLOCK: &str = "\u{E19A}";
pub const CLOUD_SLASH: &str = "\u{E1B6}";
pub const COPY: &str = "\u{E1CA}";
pub const CORNERS_OUT: &str = "\u{E1D0}";
pub const CPU: &str = "\u{E610}";
pub const EYE: &str = "\u{E220}";
pub const EYE_SLASH: &str = "\u{E224}";
pub const FOLDER_OPEN: &str = "\u{E256}";
pub const GEAR_SIX: &str = "\u{E272}";
pub const HASH: &str = "\u{E2A2}";
pub const HEADPHONES: &str = "\u{E2A6}";
pub const HOURGLASS_MEDIUM: &str = "\u{E2B8}";
pub const IDENTIFICATION_BADGE: &str = "\u{E6F6}";
pub const INFINITY: &str = "\u{E634}";
pub const INFO: &str = "\u{E2CE}";
pub const KEY: &str = "\u{E2D6}";
pub const KEYBOARD: &str = "\u{E2D8}";
pub const LIST: &str = "\u{E2F0}";
pub const LOCK_KEY: &str = "\u{E2FE}";
pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
pub const MICROPHONE: &str = "\u{E326}";
pub const MICROPHONE_SLASH: &str = "\u{E328}";
pub const MINUS: &str = "\u{E32A}";
pub const MONITOR: &str = "\u{E32E}";
pub const MONITOR_PLAY: &str = "\u{E58C}";
pub const MOON: &str = "\u{E330}";
pub const PAPER_PLANE_RIGHT: &str = "\u{E396}";
pub const PENCIL_SIMPLE: &str = "\u{E3B4}";
pub const PLUGS: &str = "\u{EB56}";
pub const PLUS: &str = "\u{E3D4}";
pub const PROHIBIT: &str = "\u{E3DE}";
pub const SCREENCAST: &str = "\u{E404}";
pub const SHIELD: &str = "\u{E40A}";
pub const SHIELD_STAR: &str = "\u{EC34}";
pub const SIGN_IN: &str = "\u{E428}";
pub const SIGN_OUT: &str = "\u{E42A}";
pub const SLIDERS_HORIZONTAL: &str = "\u{E434}";
pub const SPEAKER_HIGH: &str = "\u{E44A}";
pub const SPEAKER_SIMPLE_SLASH: &str = "\u{E456}";
pub const STAR: &str = "\u{E46A}";
pub const SUN: &str = "\u{E472}";
pub const TERMINAL_WINDOW: &str = "\u{EAE8}";
pub const TRASH: &str = "\u{E4A6}";
pub const USER: &str = "\u{E4C2}";
pub const USERS_THREE: &str = "\u{E68E}";
pub const USER_MINUS: &str = "\u{E4CE}";
pub const USER_PLUS: &str = "\u{E4D0}";
pub const WARNING: &str = "\u{E4E0}";
pub const WARNING_CIRCLE: &str = "\u{E4E2}";
pub const X: &str = "\u{E4F6}";

pub const FILL_ARROW_CIRCLE_DOWN: &str = "\u{E028}";
pub const FILL_CHECK_CIRCLE: &str = "\u{E184}";
pub const FILL_CIRCLE: &str = "\u{E18A}";
pub const FILL_CROWN_SIMPLE: &str = "\u{E616}";
pub const FILL_GAME_CONTROLLER: &str = "\u{E26E}";
pub const FILL_HAMMER: &str = "\u{E80E}";
pub const FILL_HEART: &str = "\u{E2A8}";
pub const FILL_LIGHTNING: &str = "\u{E2DE}";
pub const FILL_LOCK_SIMPLE: &str = "\u{E308}";
pub const FILL_MEGAPHONE: &str = "\u{E324}";
pub const FILL_MICROPHONE: &str = "\u{E326}";
pub const FILL_MUSIC_NOTE: &str = "\u{E33C}";
pub const FILL_PAPER_PLANE_RIGHT: &str = "\u{E396}";
pub const FILL_ROCKET_LAUNCH: &str = "\u{E3FE}";
pub const FILL_SHIELD_STAR: &str = "\u{EC34}";
pub const FILL_SHIELD_WARNING: &str = "\u{E412}";
pub const FILL_STAR: &str = "\u{E46A}";
pub const FILL_STOP: &str = "\u{E46C}";
pub const FILL_WARNING_CIRCLE: &str = "\u{E4E2}";
pub const FILL_WRENCH: &str = "\u{E5D4}";
pub const FILL_X_CIRCLE: &str = "\u{E4F8}";
