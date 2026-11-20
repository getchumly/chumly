use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use shared::ChannelInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Server name shown in the client.
    pub name: String,
    /// Message of the day, shown on connect.
    pub motd: String,
    /// Address and UDP port to listen on.
    pub bind: String,
    /// Shared server password. Empty means no password.
    pub password: String,
    pub max_users: u32,
    /// Maximum simultaneous connections from one IP address
    /// (one IPv6 /64), including ones still logging in.
    pub max_connections_per_ip: u32,
    /// How many recent messages to keep in memory per channel.
    pub history_size: usize,
    /// Answer status queries before login: name, version and the number of
    /// users online (no nicknames). Off: clients see only that the server is up.
    pub public_status: bool,
    /// At startup, ask GitHub for a newer release and say so in the log.
    pub check_updates: bool,
    pub channels: Vec<ChannelConfig>,
    /// Where the config was loaded from. Channels changed by
    /// the admin are saved back here.
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub max_users: Option<u32>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: format!("{} Server", shared::APP_NAME),
            motd: "Welcome!".into(),
            bind: format!("0.0.0.0:{}", shared::DEFAULT_PORT),
            password: String::new(),
            max_users: 64,
            max_connections_per_ip: 5,
            history_size: 100,
            public_status: true,
            check_updates: true,
            channels: vec![
                ChannelConfig { name: "General".into(), description: "Main channel".into(), max_users: None },
                ChannelConfig { name: "Games".into(), description: String::new(), max_users: None },
                ChannelConfig { name: "Meeting room".into(), description: "Up to 5 people".into(), max_users: Some(5) },
                ChannelConfig { name: "AFK".into(), description: "Away".into(), max_users: None },
            ],
            path: PathBuf::new(),
        }
    }
}

const HEADER: &str = "\
# Chumly server configuration.
# Generated automatically on first run. Restart the server after editing.
# Channels can also be changed from the client as admin: the server
# rewrites the [[channels]] section and leaves the rest of the file intact.
#
# name           server name
# motd           message shown on join
# bind           address and UDP port; forward this port on your router for internet access
# password       shared join password; empty = no password
# max_users      maximum users on the server
# max_connections_per_ip  simultaneous connections from one IP; raise it if many
#                people connect from behind one router or office NAT
# history_size   how many messages to keep in memory per channel
# public_status  let clients see the server name, version and how many people are
#                online before connecting (no nicknames); false = only whether it is up
# check_updates  at startup, ask GitHub for a newer release and write it to the log
#                (GitHub sees the server's IP address; nothing else is sent)
# [[channels]]   channel list; the first channel is the default one;
#                a channel's max_users limits how many people can be in it

";

impl ServerConfig {
    /// Loads the config. If the file does not exist, creates it with defaults.
    pub fn load_or_create(path: &Path) -> anyhow::Result<(Self, bool)> {
        if path.exists() {
            let text = std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
            let mut cfg: Self = toml::from_str(&text).with_context(|| format!("error in {}", path.display()))?;
            cfg.validate()?;
            cfg.path = path.to_path_buf();
            return Ok((cfg, false));
        }
        let cfg = Self { path: path.to_path_buf(), ..Self::default() };
        let body = toml::to_string_pretty(&cfg)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        // Private: it holds the server password.
        shared::fsutil::write_private(path, format!("{HEADER}{body}").as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok((cfg, true))
    }

    /// Stores the new channel list and saves it to the file.
    /// Write errors are only logged: the channels are already changed in memory.
    pub fn save_channels(&mut self, channels: &[ChannelInfo]) {
        self.channels = channels
            .iter()
            .map(|c| ChannelConfig { name: c.name.clone(), description: c.description.clone(), max_users: c.max_users })
            .collect();
        if let Err(e) = self.write_channels() {
            tracing::error!("failed to save channels to {}: {e:#}", self.path.display());
        }
    }

    /// Sets the server password (empty: none) and saves it to the file. The
    /// password changes only if the file was written, so what is in force is
    /// always what is on disk.
    pub fn set_password(&mut self, password: &str) -> anyhow::Result<()> {
        self.edit_file(|doc| doc["password"] = toml_edit::value(password))?;
        self.password = password.to_string();
        Ok(())
    }

    fn write_channels(&self) -> anyhow::Result<()> {
        use toml_edit::{ArrayOfTables, Item, Table, value};

        let mut list = ArrayOfTables::new();
        for c in &self.channels {
            let mut t = Table::new();
            t["name"] = value(c.name.as_str());
            if !c.description.is_empty() {
                t["description"] = value(c.description.as_str());
            }
            if let Some(m) = c.max_users {
                t["max_users"] = value(i64::from(m));
            }
            list.push(t);
        }
        self.edit_file(|doc| doc["channels"] = Item::ArrayOfTables(list))
    }

    /// Changes the file in place, keeping everything else in it (comments,
    /// other settings). No path (tests): nothing to write.
    fn edit_file(&self, edit: impl FnOnce(&mut toml_edit::DocumentMut)) -> anyhow::Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        // The file may have been deleted or corrupted while the server was running:
        // in that case rewrite it from scratch.
        let mut doc = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
            .unwrap_or_else(|| {
                let body = toml::to_string_pretty(self).unwrap_or_default();
                format!("{HEADER}{body}").parse().unwrap_or_default()
            });
        edit(&mut doc);
        // Atomic, so a failed write leaves the old config intact; private, as
        // it holds the server password.
        shared::fsutil::write_private(&self.path, doc.to_string().as_bytes())
            .with_context(|| format!("writing {}", self.path.display()))
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.channels.is_empty() {
            anyhow::bail!("config must contain at least one [[channels]] entry");
        }
        if self.max_connections_per_ip == 0 {
            anyhow::bail!("max_connections_per_ip must be at least 1");
        }
        if self.channels.iter().any(|c| c.name.trim().is_empty()) {
            anyhow::bail!("every channel must have a non-empty name");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_channels_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            "# my comment
name = \"Test\"
motd = \"hi\"

[[channels]]
name = \"Old\"
",
        )
        .unwrap();
        let (mut cfg, created) = ServerConfig::load_or_create(&path).unwrap();
        assert!(!created);

        let channels = vec![
            ChannelInfo { id: 1, name: "General".into(), description: String::new(), max_users: None },
            ChannelInfo { id: 5, name: "Quiet".into(), description: "up to three".into(), max_users: Some(3) },
        ];
        cfg.save_channels(&channels);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my comment"));
        assert!(!text.contains("Old"));
        let (again, _) = ServerConfig::load_or_create(&path).unwrap();
        assert_eq!(again.name, "Test");
        assert_eq!(again.motd, "hi");
        assert_eq!(again.channels.len(), 2);
        assert_eq!(again.channels[1].name, "Quiet");
        assert_eq!(again.channels[1].description, "up to three");
        assert_eq!(again.channels[1].max_users, Some(3));
        assert_eq!(again.channels[0].max_users, None);
    }

    #[test]
    fn set_password_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            "# my comment
name = \"Test\"

[[channels]]
name = \"General\"
",
        )
        .unwrap();
        let (mut cfg, _) = ServerConfig::load_or_create(&path).unwrap();
        cfg.set_password("пароль с пробелом").unwrap();
        assert_eq!(cfg.password, "пароль с пробелом");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my comment"), "{text}");
        let (again, _) = ServerConfig::load_or_create(&path).unwrap();
        assert_eq!(again.password, "пароль с пробелом");
        assert_eq!(again.name, "Test");
        assert_eq!(again.channels[0].name, "General");
        // Removed: an empty password, anyone can join.
        cfg.set_password("").unwrap();
        assert_eq!(ServerConfig::load_or_create(&path).unwrap().0.password, "");
    }
}
