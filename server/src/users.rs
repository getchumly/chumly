//! Registered users and the server's roles (users.toml): a key with its
//! registered nickname and role, and the role list in hierarchy order (the
//! first is the most senior). The key is the SHA-256 of the client's public
//! key, see `tls::client_key`.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use shared::{OWNER_ROLE, Perms, RoleId, RoleInfo, Roles};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registered {
    pub key: String,
    /// Nobody else can use this nickname. Compared case-insensitively.
    pub nickname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<RoleId>,
    pub created: i64,
}

/// A role as written in users.toml: rights by name, so the file stays
/// readable and editable by hand.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoleDef {
    id: RoleId,
    name: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    permissions: Vec<String>,
}

impl From<&RoleInfo> for RoleDef {
    fn from(r: &RoleInfo) -> Self {
        Self {
            id: r.id,
            name: r.name.clone(),
            color: r.color.clone(),
            icon: r.icon.clone(),
            permissions: r.perms.names().into_iter().map(String::from).collect(),
        }
    }
}

impl RoleDef {
    fn into_info(self) -> RoleInfo {
        let mut perms = Perms::NONE;
        for name in &self.permissions {
            match Perms::from_name(name) {
                Some(p) => perms = perms | p,
                None => tracing::warn!("users.toml: role {}: unknown permission '{name}' ignored", self.name),
            }
        }
        RoleInfo { id: self.id, name: self.name, color: self.color, icon: self.icon, perms }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    roles: Vec<RoleDef>,
    #[serde(default)]
    users: Vec<Registered>,
}

#[derive(Debug, Default)]
pub struct Registry {
    pub roles: Roles,
    pub users: Vec<Registered>,
    path: PathBuf,
}

impl Registry {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file: File = if path.exists() {
            toml::from_str(&std::fs::read_to_string(path)?).with_context(|| format!("error in {}", path.display()))?
        } else {
            File::default()
        };
        let mut r = Registry {
            roles: Roles { list: file.roles.into_iter().map(RoleDef::into_info).collect() },
            users: file.users,
            path: path.to_path_buf(),
        };
        if r.fix_roles() {
            r.save();
        }
        Ok(r)
    }

    /// Makes the role list valid: the default roles if there are none, the
    /// owner role first, no duplicate ids, no users with a missing role.
    /// Returns whether anything changed.
    fn fix_roles(&mut self) -> bool {
        let before = self.roles.clone();
        if self.roles.list.is_empty() {
            self.roles = Roles::defaults();
        }
        let mut seen = std::collections::HashSet::new();
        self.roles.list.retain(|r| r.id != 0 && seen.insert(r.id));
        match self.roles.index(OWNER_ROLE) {
            Some(0) => {}
            Some(i) => {
                let owner = self.roles.list.remove(i);
                self.roles.list.insert(0, owner);
            }
            None => self.roles.list.insert(0, Roles::defaults().list.remove(0)),
        }
        self.roles.list[0].perms = Perms::ALL;
        let mut users_changed = false;
        for u in &mut self.users {
            if u.role.is_some_and(|id| self.roles.get(id).is_none()) {
                tracing::warn!("users.toml: {} had a role that does not exist, now none", u.nickname);
                u.role = None;
                users_changed = true;
            }
        }
        users_changed || self.roles != before
    }

    pub fn owner(&self) -> Option<&Registered> {
        self.users.iter().find(|u| u.role == Some(OWNER_ROLE))
    }

    /// Makes `key` (registered to `nickname` if it is not yet) the only
    /// owner; previous owners get the most senior role after the owner's.
    /// Returns their keys.
    pub fn set_owner(&mut self, key: &str, nickname: &str) -> Vec<String> {
        let next = self.roles.list.get(1).map(|r| r.id);
        let mut old = Vec::new();
        for r in self.users.iter_mut().filter(|r| r.role == Some(OWNER_ROLE) && r.key != key) {
            r.role = next;
            old.push(r.key.clone());
        }
        self.register(key, nickname).role = Some(OWNER_ROLE);
        old
    }

    /// Written through a temp file, so a failed write keeps the old list.
    pub fn save(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        let file = File { roles: self.roles.list.iter().map(Into::into).collect(), users: self.users.clone() };
        let res = toml::to_string_pretty(&file).map_err(anyhow::Error::from).and_then(|s| {
            let tmp = self.path.with_extension("toml.tmp");
            std::fs::write(&tmp, s)?;
            std::fs::rename(&tmp, &self.path)?;
            Ok(())
        });
        if let Err(e) = res {
            tracing::error!("failed to save {}: {e:#}", self.path.display());
        }
    }

    pub fn by_key(&self, key: &str) -> Option<&Registered> {
        self.users.iter().find(|u| u.key == key)
    }

    pub fn by_nick(&self, nick: &str) -> Option<&Registered> {
        let nick = nick.to_lowercase();
        self.users.iter().find(|u| u.nickname.to_lowercase() == nick)
    }

    /// Registers `nickname` to `key`, or renames the existing registration.
    /// The caller checks that the nickname is free.
    pub fn register(&mut self, key: &str, nickname: &str) -> &mut Registered {
        let i = match self.users.iter().position(|u| u.key == key) {
            Some(i) => {
                self.users[i].nickname = nickname.to_string();
                i
            }
            None => {
                self.users.push(Registered {
                    key: key.to_string(),
                    nickname: nickname.to_string(),
                    role: None,
                    created: shared::unix_now(),
                });
                self.users.len() - 1
            }
        };
        &mut self.users[i]
    }

    pub fn remove_key(&mut self, key: &str) -> Option<Registered> {
        let i = self.users.iter().position(|u| u.key == key)?;
        Some(self.users.remove(i))
    }

    /// A role by name, case-insensitively (console commands).
    pub fn role_by_name(&self, name: &str) -> Option<&RoleInfo> {
        let name = name.to_lowercase();
        self.roles.list.iter().find(|r| r.name.to_lowercase() == name)
    }

    /// Adds a role at the bottom of the list; returns its id.
    pub fn add_role(&mut self, mut role: RoleInfo) -> RoleId {
        role.id = self.roles.list.iter().map(|r| r.id).max().unwrap_or(OWNER_ROLE) + 1;
        let id = role.id;
        self.roles.list.push(role);
        id
    }

    /// Removes a role and takes it from its users; returns their keys.
    pub fn remove_role(&mut self, id: RoleId) -> Vec<String> {
        self.roles.list.retain(|r| r.id != id);
        let mut keys = Vec::new();
        for u in self.users.iter_mut().filter(|u| u.role == Some(id)) {
            u.role = None;
            keys.push(u.key.clone());
        }
        keys
    }

    /// Moves a role to `index` (clamped below the owner role).
    pub fn move_role(&mut self, id: RoleId, index: usize) {
        let Some(from) = self.roles.index(id) else { return };
        let role = self.roles.list.remove(from);
        let index = index.clamp(1, self.roles.list.len());
        self.roles.list.insert(index, role);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_rename_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("users.toml");
        let mut r = Registry::load(&path).unwrap();
        assert_eq!(r.roles, Roles::defaults(), "a new file gets the default roles");
        r.register("K1", "Alice").role = Some(OWNER_ROLE);
        r.register("K2", "bob").role = Some(3);
        assert_eq!(r.by_nick("alice").unwrap().key, "K1");
        r.register("K1", "Alicia");
        assert!(r.by_nick("alice").is_none());
        let helper = r.add_role(RoleInfo {
            id: 0,
            name: "Helper".into(),
            color: "teal".into(),
            icon: "heart".into(),
            perms: Perms::KICK | Perms::CHANNELS,
        });
        assert_eq!(helper, 4);
        r.move_role(helper, 1);
        r.save();

        let back = Registry::load(&path).unwrap();
        assert_eq!(back.users.len(), 2);
        assert_eq!(back.owner().unwrap().nickname, "Alicia");
        assert_eq!(back.by_key("K2").unwrap().role, Some(3));
        assert_eq!(back.roles, r.roles);
        assert_eq!(back.roles.list[1].name, "Helper");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("permissions = [\n    \"kick\",\n    \"channels\",\n]"), "{text}");
    }

    #[test]
    fn set_owner_moves_the_role() {
        let mut r = Registry { roles: Roles::defaults(), ..Default::default() };
        r.register("K1", "first").role = Some(OWNER_ROLE);
        r.register("K2", "mod").role = Some(3);
        assert_eq!(r.set_owner("K3", "next"), vec!["K1".to_string()]);
        assert_eq!(r.owner().unwrap().nickname, "next");
        assert_eq!(r.by_key("K1").unwrap().role, Some(2), "the previous owner gets the next role");
        assert_eq!(r.by_key("K2").unwrap().role, Some(3));
        assert_eq!(r.users.iter().filter(|u| u.role == Some(OWNER_ROLE)).count(), 1);
        // An existing registration keeps its entry and gets the role.
        assert_eq!(r.set_owner("K2", "mod"), vec!["K3".to_string()]);
        assert_eq!(r.owner().unwrap().key, "K2");
    }

    #[test]
    fn missing_and_broken_role_lists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("users.toml");
        // No role list: the default roles, saved to the file.
        std::fs::write(&path, "[[users]]\nkey = \"K1\"\nnickname = \"first\"\nrole = 1\ncreated = 100\n").unwrap();
        let r = Registry::load(&path).unwrap();
        assert_eq!(r.owner().unwrap().key, "K1");
        assert_eq!(r.roles, Roles::defaults());
        assert!(std::fs::read_to_string(&path).unwrap().contains("[[roles]]"), "roles saved");

        // The owner role out of place and a user with a deleted role.
        std::fs::write(
            &path,
            "[[roles]]\nid = 5\nname = \"Mod\"\npermissions = [\"kick\", \"fly\"]\n\n\
             [[roles]]\nid = 1\nname = \"Boss\"\n\n\
             [[users]]\nkey = \"K1\"\nnickname = \"a\"\nrole = 9\ncreated = 1\n",
        )
        .unwrap();
        let r = Registry::load(&path).unwrap();
        assert_eq!(r.roles.list.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 5]);
        assert_eq!(r.roles.list[0].perms, Perms::ALL);
        assert_eq!(r.roles.list[1].perms, Perms::KICK, "unknown permission names are ignored");
        assert_eq!(r.by_key("K1").unwrap().role, None);
    }

    #[test]
    fn removing_a_role_takes_it_from_users() {
        let mut r = Registry { roles: Roles::defaults(), ..Default::default() };
        r.register("K1", "a").role = Some(3);
        r.register("K2", "b").role = Some(2);
        assert_eq!(r.remove_role(3), vec!["K1".to_string()]);
        assert_eq!(r.by_key("K1").unwrap().role, None);
        assert_eq!(r.by_key("K2").unwrap().role, Some(2));
        // Moving never goes above the owner.
        r.move_role(2, 0);
        assert_eq!(r.roles.list[0].id, OWNER_ROLE);
    }
}
