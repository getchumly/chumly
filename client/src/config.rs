use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::screen::Quality;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Ru,
    En,
}

impl Language {
    /// The system's language, for the first start (no client.toml yet):
    /// Russian on a Russian system, English otherwise. Windows reports the
    /// UI language; Linux the LC_ALL / LC_MESSAGES / LANG variables.
    pub fn system() -> Self {
        sys_locale::get_locale().as_deref().map_or(Self::En, Self::from_locale)
    }

    /// "ru", "ru-RU", "ru_RU.UTF-8" are Russian; everything else English.
    fn from_locale(tag: &str) -> Self {
        let lang = tag.split(['-', '_', '.', '@']).next().unwrap_or("");
        if lang.eq_ignore_ascii_case("ru") { Self::Ru } else { Self::En }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    VoiceActivation,
    PushToTalk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// `None` means the default device.
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub mode: VoiceMode,
    /// Key name in device_query format, e.g. "Grave" or "LControl".
    pub ptt_key: String,
    /// Voice activation threshold in dBFS.
    pub vad_threshold_db: f32,
    pub noise_suppression: bool,
    pub input_gain: f32,
    pub output_volume: f32,
    pub event_sounds: bool,
    /// Volume of event sounds, 0..=1, on top of the output volume.
    pub notify_volume: f32,
    /// Which event sounds play (all of them by default).
    pub sound_groups: SoundGroups,
}

/// Event sound groups that can be silenced one by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundGroups {
    /// Connecting to and disconnecting from the server.
    pub server: bool,
    /// Someone joins or leaves my channel (and my own moves).
    pub channel: bool,
    /// Private messages and server notices.
    pub messages: bool,
    /// My own mute and deafen toggles.
    pub mic_sound: bool,
    /// A screen share in my channel starts or ends.
    pub stream: bool,
    /// Viewers of a share join or leave: mine as the streamer, or my own watching.
    pub viewers: bool,
}

impl Default for SoundGroups {
    fn default() -> Self {
        Self { server: true, channel: true, messages: true, mic_sound: true, stream: true, viewers: true }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            mode: VoiceMode::VoiceActivation,
            ptt_key: "Grave".into(),
            vad_threshold_db: -45.0,
            noise_suppression: true,
            input_gain: 1.0,
            output_volume: 1.0,
            event_sounds: true,
            notify_volume: 0.7,
            sound_groups: SoundGroups::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerBookmark {
    pub name: String,
    pub address: String,
    pub nickname: String,
    /// Stored in plain text, like most voice chat clients do.
    #[serde(default)]
    pub password: String,
    /// Certificate fingerprint pinned on first connect.
    #[serde(default)]
    pub cert_fingerprint: Option<String>,
    /// Pinned to the favorites group at the top of the list. The list keeps
    /// favorites first (`ClientConfig::sort_favorites_first`).
    #[serde(default)]
    pub favorite: bool,
    /// Unix time of the last successful connect.
    #[serde(default)]
    pub last_connected: Option<i64>,
}

/// Window position and size in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl WindowGeometry {
    /// Discards obviously broken values, e.g. a window far off-screen
    /// after a second monitor was disconnected.
    pub fn sane(self) -> Option<Self> {
        let ok = self.width >= 200.0
            && self.height >= 150.0
            && self.width <= 16_000.0
            && self.height <= 16_000.0
            && self.x.abs() < 20_000.0
            && self.y.abs() < 20_000.0;
        ok.then_some(self)
    }
}

/// Global hotkeys, e.g. "Ctrl+Shift+M". Empty means unassigned.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub toggle_mute: String,
    pub toggle_deafen: String,
}

/// Last choice made in the screen share dialog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareConfig {
    pub quality: Quality,
    pub audio: bool,
    /// The thumbnail of our own share in the "me" block.
    pub preview: bool,
}

impl Default for ShareConfig {
    fn default() -> Self {
        Self { quality: Quality::P1080F30, audio: true, preview: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub language: Language,
    pub theme: Theme,
    /// The OS draws the window frame and title bar. Otherwise Chumly draws
    /// its own controls in the top bar. Default: own frame on Windows,
    /// system frame elsewhere (Linux desktops and tiling WMs differ too much).
    pub system_frame: bool,
    /// On start, connect to the server used last.
    pub auto_connect: bool,
    /// Ask GitHub for a newer release at launch and once a day. "Check now"
    /// works either way.
    pub check_updates: bool,
    /// "Skip this version": no notice for it until a newer one comes out.
    pub skipped_version: Option<String>,
    /// The version that last ran: a newer one now means it was updated
    /// ("Updated to …" on the start screen).
    pub last_version: Option<String>,
    /// The crash report whose banner was closed (its file name).
    pub crash_dismissed: Option<String>,
    pub audio: AudioConfig,
    pub servers: Vec<ServerBookmark>,
    /// Per-user volume by nickname, 0.0 to 2.0.
    pub user_volumes: BTreeMap<String, f32>,
    /// Screen share audio volume by sharer nickname, 0.0 to 2.0.
    pub stream_volumes: BTreeMap<String, f32>,
    pub share: ShareConfig,
    pub hotkeys: HotkeyConfig,
    /// Main window.
    pub window: Option<WindowGeometry>,
    /// Screen share viewer window.
    pub viewer_window: Option<WindowGeometry>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            language: Language::system(),
            theme: Theme::Dark,
            system_frame: !cfg!(windows),
            auto_connect: false,
            check_updates: true,
            skipped_version: None,
            last_version: None,
            crash_dismissed: None,
            audio: AudioConfig::default(),
            // The empty state invites to add one; no placeholder bookmark.
            servers: Vec::new(),
            user_volumes: BTreeMap::new(),
            stream_volumes: BTreeMap::new(),
            share: ShareConfig::default(),
            hotkeys: HotkeyConfig::default(),
            window: None,
            viewer_window: None,
        }
    }
}

impl ClientConfig {
    /// Favorites first, otherwise the user's order (a stable sort).
    pub fn sort_favorites_first(&mut self) {
        self.servers.sort_by_key(|b| !b.favorite);
    }

    /// Where a new bookmark goes: the top of the non-favorite group.
    pub fn new_bookmark_index(&self) -> usize {
        self.servers.iter().take_while(|b| b.favorite).count()
    }

    /// Moves bookmark `from` so it ends up at `slot` among the others, and
    /// puts it in the group of that place. At the boundary between the
    /// groups it keeps its own group.
    pub fn move_bookmark(&mut self, from: usize, slot: usize) {
        if from >= self.servers.len() {
            return;
        }
        let mut bm = self.servers.remove(from);
        let favorites = self.servers.iter().filter(|b| b.favorite).count();
        let slot = slot.min(self.servers.len());
        if slot != favorites {
            bm.favorite = slot < favorites;
        }
        self.servers.insert(slot, bm);
        self.sort_favorites_first();
    }

    /// Stars or unstars a bookmark: a new favorite goes to the end of the
    /// favorites, an unstarred one to the top of the others.
    pub fn toggle_favorite(&mut self, i: usize) {
        if i >= self.servers.len() {
            return;
        }
        let mut bm = self.servers.remove(i);
        bm.favorite = !bm.favorite;
        let at = self.new_bookmark_index();
        self.servers.insert(at, bm);
    }

    /// The bookmark connected to most recently.
    pub fn last_used(&self) -> Option<usize> {
        self.servers
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.last_connected.map(|t| (i, t)))
            .max_by_key(|(_, t)| *t)
            .map(|(i, _)| i)
    }
    /// Default path. Portable mode: a client.toml next to the executable
    /// wins. Otherwise %APPDATA%\Chumly\config\client.toml on Windows,
    /// ~/.config/chumly/client.toml on Linux, a folder under
    /// ~/Library/Application Support/ on macOS.
    pub fn default_path() -> Option<PathBuf> {
        if let Some(portable) = Self::portable_path() {
            return Some(portable);
        }
        let path = directories::ProjectDirs::from("", "", shared::APP_NAME)?.config_dir().join("client.toml");
        Some(path)
    }

    /// client.toml next to the executable, if it exists.
    fn portable_path() -> Option<PathBuf> {
        let path = std::env::current_exe().ok()?.parent()?.join("client.toml");
        path.is_file().then_some(path)
    }

    pub fn load(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::error!("error in {}: {e}. Using default settings.", path.display());
                    // Keep the broken file so it is not lost when overwritten.
                    let _ = shared::fsutil::write_private(&path.with_extension("toml.bak"), text.as_bytes());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        // Atomic, so a crash mid-write cannot leave a truncated config; private,
        // because it holds server passwords.
        shared::fsutil::write_private(path, text.as_bytes()).with_context(|| format!("writing {}", path.display()))
    }

    pub fn stream_volume(&self, nick: &str) -> f32 {
        self.stream_volumes.get(nick).copied().unwrap_or(1.0)
    }

    pub fn user_volume(&self, nickname: &str) -> f32 {
        self.user_volumes.get(nickname).copied().unwrap_or(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm(name: &str, favorite: bool) -> ServerBookmark {
        ServerBookmark { name: name.into(), favorite, ..Default::default() }
    }

    fn names(c: &ClientConfig) -> Vec<(String, bool)> {
        c.servers.iter().map(|b| (b.name.clone(), b.favorite)).collect()
    }

    fn pairs(v: &[(&str, bool)]) -> Vec<(String, bool)> {
        v.iter().map(|(n, f)| (n.to_string(), *f)).collect()
    }

    #[test]
    fn bookmark_order_and_favorites() {
        let mut c = ClientConfig { servers: vec![bm("a", false), bm("F", true), bm("b", false)], ..Default::default() };
        c.sort_favorites_first();
        assert_eq!(names(&c), pairs(&[("F", true), ("a", false), ("b", false)]));
        assert_eq!(c.new_bookmark_index(), 1, "new bookmarks go under the favorites");
        // Dragged into the favorites group becomes a favorite.
        c.move_bookmark(2, 0);
        assert_eq!(names(&c), pairs(&[("b", true), ("F", true), ("a", false)]));
        // Dropped at the boundary keeps its group.
        c.move_bookmark(0, 1);
        assert_eq!(names(&c), pairs(&[("F", true), ("b", true), ("a", false)]));
        // Dragged below the favorites stops being one.
        c.move_bookmark(0, 2);
        assert_eq!(names(&c), pairs(&[("b", true), ("a", false), ("F", false)]));
        // Unstar: to the top of the others; star: to the end of the favorites.
        c.toggle_favorite(0);
        assert_eq!(names(&c), pairs(&[("b", false), ("a", false), ("F", false)]));
        c.toggle_favorite(2);
        assert_eq!(names(&c), pairs(&[("F", true), ("b", false), ("a", false)]));
    }

    #[test]
    fn last_used_is_the_latest() {
        let mut c =
            ClientConfig { servers: vec![bm("a", false), bm("b", false), bm("c", false)], ..Default::default() };
        assert_eq!(c.last_used(), None);
        c.servers[0].last_connected = Some(10);
        c.servers[2].last_connected = Some(30);
        assert_eq!(c.last_used(), Some(2));
    }

    #[test]
    fn language_from_the_system_locale() {
        for ru in ["ru", "ru-RU", "ru_RU.UTF-8", "RU-ru", "ru_UA", "ru@latin"] {
            assert_eq!(Language::from_locale(ru), Language::Ru, "{ru}");
        }
        for en in ["en-US", "en_GB.UTF-8", "uk-UA", "be-BY", "de-DE", "C", "POSIX", "", "rus", "run"] {
            assert_eq!(Language::from_locale(en), Language::En, "{en}");
        }
    }
}
