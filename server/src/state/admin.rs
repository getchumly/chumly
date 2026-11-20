//! Moderation and administration: kick/ban, giving roles and registered
//! nicknames (users.toml), the one-time setup code, the admin panel lists.

use shared::RoleId;

use super::*;

/// What happens to a user who is removed from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punish {
    /// Disconnect; they can come back.
    Kick,
    /// Disconnect and ban by nickname, IP and key, for this many seconds or,
    /// with `None`, permanently.
    Ban { secs: Option<u64> },
}

const OWNER_IS_FIXED: &str = "this is the server owner; ownership moves only with `owner <nick>`";

/// Who performs an action on other users: their role and key (`None` for
/// the console or a client without a key).
#[derive(Clone, Copy)]
pub struct Actor<'a> {
    pub role: Option<RoleId>,
    pub key: Option<&'a str>,
}

impl Actor<'static> {
    /// The server console: the owner's rights. It cannot change the owner
    /// either; that is `transfer_owner`.
    pub const CONSOLE: Self = Self { role: Some(OWNER_ROLE), key: None };
}

impl State {
    /// The one-time setup code from a client: the right one makes them the
    /// owner. A wrong one is answered with a penalty against guessing.
    pub(super) fn admin_setup(&mut self, me: &UserInfo, code: &str) -> After {
        let Some((ip, key)) = self.users.get(&me.id).map(|u| (u.addr.ip(), u.key.clone())) else {
            return After::Nothing;
        };
        let Some(key) = key else {
            self.send_error(me.id, ErrorCode::IdentityRequired);
            self.send(me.id, ServerMsg::AdminSetupResult { ok: false });
            return After::Nothing;
        };
        let now = Instant::now();
        let blocked = !self.auth.may_check(ip, Secret::Admin, now);
        let ok = !blocked
            && self.setup_code.as_deref().is_some_and(|c| secret_eq(&normalize_code(code), &normalize_code(c)));
        self.send(me.id, ServerMsg::AdminSetupResult { ok });
        if !ok {
            info!(
                "{} ({ip}): wrong admin setup code{}",
                me.nickname,
                if blocked { " (not checked: address blocked or server under attack)" } else { "" }
            );
            if !blocked && self.auth.record_failure(ip, now) {
                warn!("{ip}: too many wrong admin codes, address blocked");
            }
            return After::Penalty;
        }
        self.auth.record_success(ip, Secret::Admin);
        self.setup_code = None;
        self.registry.register(&key, &me.nickname).role = Some(OWNER_ROLE);
        self.registry.save();
        warn!("admin setup: {} ({}) is now the owner, the setup code is used up", me.nickname, short_key(&key));
        self.update_user(me.id, |u| {
            u.role = Some(OWNER_ROLE);
            u.registered = true;
        });
        After::Nothing
    }

    /// Kick or ban from a client: only users strictly below, see
    /// `Roles::outranks`.
    pub(super) fn moderate(
        &mut self,
        me: &UserInfo,
        target: UserId,
        reason: String,
        punish: Punish,
    ) -> Result<(), ErrorCode> {
        if target == me.id {
            return Err(ErrorCode::CannotTargetSelf);
        }
        let target_role = self.users.get(&target).and_then(|u| u.info.role);
        if !self.registry.roles.outranks(me.role, target_role) {
            return Err(ErrorCode::NotAdmin);
        }
        if !self.users.contains_key(&target) {
            return Err(ErrorCode::NoSuchUser);
        }
        self.kick(target, reason, punish, Some(&me.nickname));
        Ok(())
    }

    pub(super) fn unban(&mut self, ban: &shared::BanInfo, by: &str) -> Result<(), ErrorCode> {
        if !self.bans.remove_exact(ban) {
            return Err(ErrorCode::NoSuchUser);
        }
        info!("{by}: unban {} ({})", ban.nickname.as_deref().unwrap_or("-"), ban.ip.as_deref().unwrap_or("-"));
        Ok(())
    }

    /// The users tab of the admin panel.
    pub(super) fn send_panel_users(&self, id: UserId) {
        self.send(id, ServerMsg::ServerUsers { users: self.panel_users() });
    }

    /// Sets the server password, empty removes it; saved to server.toml. Who
    /// may do it (the owner, the console) is checked by the caller. Users
    /// already connected stay.
    pub fn set_password(&mut self, password: &str, by: &str) -> Result<(), ErrorCode> {
        if !shared::validate_password(password) {
            return Err(ErrorCode::InvalidPassword);
        }
        if let Err(e) = self.config.set_password(password) {
            tracing::error!("{by}: failed to save the server password: {e:#}");
            return Err(ErrorCode::Unknown);
        }
        info!("{by}: server password {}", if password.is_empty() { "removed" } else { "changed" });
        Ok(())
    }

    /// The server settings tab of the owner's panel.
    pub(super) fn send_server_settings(&self, id: UserId) {
        self.send(id, ServerMsg::ServerSettings { password_set: !self.config.password.is_empty() });
    }

    /// The bans tab of the admin panel.
    pub(super) fn send_bans(&self, id: UserId) {
        self.send(id, ServerMsg::Bans { bans: self.bans.infos() });
    }

    /// Kicks the user and, with `Punish::Ban`, bans them by nickname, IP and
    /// key. `by`: the moderator's nickname, `None` for the console.
    pub fn kick(&mut self, target: UserId, reason: String, punish: Punish, by: Option<&str>) -> bool {
        let Some(u) = self.users.get(&target) else { return false };
        let now = shared::unix_now();
        let (banned, expires) = match punish {
            Punish::Kick => (false, None),
            Punish::Ban { secs } => (true, crate::bans::expiry(now, secs)),
        };
        // The duration as stored (clamped), so the user sees the real one.
        let ban_secs = expires.map(|e| (e - now) as u64);
        u.push(ServerMsg::Kicked { reason: reason.clone(), banned, ban_secs });
        let (nick, ip, key) = (u.info.nickname.clone(), u.addr.ip(), u.key.clone());
        let what = match (banned, ban_secs) {
            (false, _) => "kick".to_string(),
            (true, None) => "ban".to_string(),
            (true, Some(s)) => format!("ban for {}", crate::bans::fmt_remaining(s)),
        };
        info!(
            "{}: {what} {nick} ({ip}), reason: {}",
            by.unwrap_or("console"),
            if reason.is_empty() { "-" } else { &reason }
        );
        if banned {
            self.bans.add(Ban {
                nickname: Some(nick),
                ip: Some(ip),
                key,
                reason,
                created: now,
                expires,
                by: by.map(String::from),
            });
        }
        // Removing the user closes the send channel. The writer task delivers
        // the Kicked message and closes the connection.
        self.unregister(target);
        true
    }

    /// Online user with this id and their key, or the right error.
    pub(super) fn keyed_user(&self, user_id: UserId) -> Result<(String, String), ErrorCode> {
        let u = self.users.get(&user_id).ok_or(ErrorCode::NoSuchUser)?;
        let key = u.key.clone().ok_or(ErrorCode::IdentityRequired)?;
        Ok((key, u.info.nickname.clone()))
    }

    /// Who acts: their role and key. The console acts with the owner's
    /// rights (it can do anything except change the owner, see `transfer_owner`).
    /// Checks the hierarchy rule against the target: only strictly below,
    /// never oneself.
    pub(super) fn check_rank(&self, actor: Actor<'_>, target_key: &str) -> Result<Option<RoleId>, ErrorCode> {
        if actor.key == Some(target_key) {
            return Err(ErrorCode::CannotTargetSelf);
        }
        let target = self.registry.by_key(target_key).and_then(|r| r.role);
        if !self.registry.roles.outranks(actor.role, target) {
            return Err(ErrorCode::NotAdmin);
        }
        Ok(target)
    }

    /// Updates the online users with this key after a registry change.
    pub(super) fn refresh_user(&mut self, key: &str) {
        let reg = self.registry.by_key(key).cloned();
        let ids: Vec<UserId> =
            self.users.values().filter(|u| u.key.as_deref() == Some(key)).map(|u| u.info.id).collect();
        for id in ids {
            self.update_user(id, |u| {
                u.role = reg.as_ref().and_then(|r| r.role);
                u.registered = reg.is_some();
            });
        }
    }

    /// Gives an online user a role (`None` takes it away), registering
    /// their nickname to their key if needed.
    pub fn set_role(
        &mut self,
        user_id: UserId,
        role: Option<RoleId>,
        actor: Actor<'_>,
        by: &str,
    ) -> Result<(), ErrorCode> {
        let (key, nick) = self.keyed_user(user_id)?;
        self.check_rank(actor, &key)?;
        self.check_assign(actor, role)?;
        self.registry.register(&key, &nick).role = role;
        self.registry.save();
        info!("{by}: role of {nick} ({}) set to {}", short_key(&key), self.role_name(role));
        self.refresh_user(&key);
        Ok(())
    }

    fn check_assign(&self, actor: Actor<'_>, role: Option<RoleId>) -> Result<(), ErrorCode> {
        if role.is_some_and(|id| self.registry.roles.get(id).is_none()) {
            return Err(ErrorCode::NoSuchRole);
        }
        if !self.registry.roles.can_assign(actor.role, role) {
            return Err(ErrorCode::NotAdmin);
        }
        Ok(())
    }

    /// Role name for logs and the console.
    pub fn role_name(&self, role: Option<RoleId>) -> String {
        match role {
            None => "none".into(),
            Some(id) => self.registry.roles.get(id).map_or_else(|| format!("#{id}"), |r| r.name.clone()),
        }
    }

    /// Registers an online user's nickname to their key, or removes the
    /// registration together with the role.
    pub fn set_registered(
        &mut self,
        user_id: UserId,
        registered: bool,
        actor: Actor<'_>,
        by: &str,
    ) -> Result<(), ErrorCode> {
        let (key, nick) = self.keyed_user(user_id)?;
        self.check_rank(actor, &key)?;
        if registered {
            self.registry.register(&key, &nick);
            info!("{by}: registered nickname {nick} to key {}", short_key(&key));
        } else {
            self.registry.remove_key(&key);
            info!("{by}: removed registration of {nick} ({})", short_key(&key));
        }
        self.registry.save();
        self.refresh_user(&key);
        Ok(())
    }

    /// Admin panel: registered users (online or not), then unregistered
    /// users who are online. Senior roles first, then by nickname.
    pub(super) fn panel_users(&self) -> Vec<shared::PanelUser> {
        let online = |key: &str| self.users.values().find(|u| u.key.as_deref() == Some(key));
        let mut list: Vec<shared::PanelUser> = self
            .registry
            .users
            .iter()
            .map(|r| shared::PanelUser {
                key: Some(r.key.clone()),
                nickname: r.nickname.clone(),
                role: r.role,
                registered: true,
                online: online(&r.key).map(|u| u.info.id),
                created: r.created,
                client_version: online(&r.key).map(|u| u.client_version.clone()).unwrap_or_default(),
            })
            .collect();
        for u in self.users.values() {
            if u.key.as_deref().is_some_and(|k| self.registry.by_key(k).is_some()) {
                continue;
            }
            list.push(shared::PanelUser {
                key: u.key.clone(),
                nickname: u.info.nickname.clone(),
                role: None,
                registered: false,
                online: Some(u.info.id),
                created: 0,
                client_version: u.client_version.clone(),
            });
        }
        list.sort_by(|a, b| {
            b.registered
                .cmp(&a.registered)
                .then_with(|| self.registry.roles.rank(b.role).cmp(&self.registry.roles.rank(a.role)))
                .then_with(|| a.nickname.to_lowercase().cmp(&b.nickname.to_lowercase()))
        });
        list
    }

    /// Admin panel: role of a registered user by key, online or not.
    pub(super) fn set_role_key(
        &mut self,
        key: &str,
        role: Option<RoleId>,
        actor: Actor<'_>,
        by: &str,
    ) -> Result<(), ErrorCode> {
        if self.registry.by_key(key).is_none() {
            return Err(ErrorCode::NoSuchUser);
        }
        self.check_rank(actor, key)?;
        self.check_assign(actor, role)?;
        let nick = match self.registry.users.iter_mut().find(|r| r.key == key) {
            Some(r) => {
                r.role = role;
                r.nickname.clone()
            }
            None => return Err(ErrorCode::NoSuchUser),
        };
        self.registry.save();
        info!("{by}: role of {nick} ({}) set to {}", short_key(key), self.role_name(role));
        self.refresh_user(key);
        Ok(())
    }

    /// Admin panel: removes a registration by key, online or not.
    pub(super) fn unregister_key(&mut self, key: &str, actor: Actor<'_>, by: &str) -> Result<(), ErrorCode> {
        if self.registry.by_key(key).is_none() {
            return Err(ErrorCode::NoSuchUser);
        }
        self.check_rank(actor, key)?;
        let r = self.registry.remove_key(key).ok_or(ErrorCode::NoSuchUser)?;
        self.registry.save();
        info!("{by}: removed registration of {} ({})", r.nickname, short_key(key));
        self.refresh_user(key);
        Ok(())
    }

    /// Console: removes a registration by nickname, online or not.
    pub fn unregister_nick(&mut self, nick: &str) -> Result<String, String> {
        let key = self.registry.by_nick(nick).map(|r| r.key.clone()).ok_or(format!("'{nick}' is not registered"))?;
        if self.registry.by_key(&key).is_some_and(|r| r.role == Some(OWNER_ROLE)) {
            return Err(OWNER_IS_FIXED.into());
        }
        let r = self.registry.remove_key(&key).expect("found above");
        self.registry.save();
        self.refresh_user(&key);
        Ok(format!("Registration of {} removed", r.nickname))
    }

    /// Console: takes the role away by nickname, online or not.
    pub fn revoke_nick(&mut self, nick: &str) -> Result<String, String> {
        let key = self.registry.by_nick(nick).map(|r| r.key.clone()).ok_or(format!("'{nick}' is not registered"))?;
        if self.registry.by_key(&key).is_some_and(|r| r.role == Some(OWNER_ROLE)) {
            return Err(OWNER_IS_FIXED.into());
        }
        if let Some(r) = self.registry.users.iter_mut().find(|r| r.key == key) {
            r.role = None;
        }
        self.registry.save();
        self.refresh_user(&key);
        Ok(format!("{nick} has no role now (the nickname stays registered)"))
    }

    /// Console: makes the online user `nick` the owner. The previous owner
    /// gets the next role down. The only way the owner role moves.
    pub fn transfer_owner(&mut self, nick: &str) -> Result<String, String> {
        let uid = self.find_by_nick(nick).ok_or(format!("'{nick}' is not online"))?;
        let (key, nick) = self.keyed_user(uid).map_err(|_| format!("{nick} has no key: the owner needs one"))?;
        if self.registry.by_key(&key).is_some_and(|r| r.role == Some(OWNER_ROLE)) {
            return Err(format!("{nick} is the owner already"));
        }
        let old = self.registry.set_owner(&key, &nick);
        self.registry.save();
        self.setup_code = None;
        warn!("console: {nick} ({}) is the owner now", short_key(&key));
        for k in &old {
            self.refresh_user(k);
        }
        self.refresh_user(&key);
        let next = self.role_name(self.registry.roles.list.get(1).map(|r| r.id));
        Ok(format!("{nick} is the owner now; the previous owner, if any, has the role {next}"))
    }

    /// Console: binds the registration of `registered` to the key of the
    /// online user `online`. For users whose key changed, e.g. after the
    /// server certificate was replaced.
    pub fn rebind(&mut self, registered: &str, online: &str) -> Result<String, String> {
        let old_key = self
            .registry
            .by_nick(registered)
            .map(|r| r.key.clone())
            .ok_or(format!("'{registered}' is not registered"))?;
        let uid = self.find_by_nick(online).ok_or(format!("'{online}' is not online"))?;
        let new_key = self.users[&uid].key.clone().ok_or(format!("{online} has no key"))?;
        if let Some(other) = self.registry.by_key(&new_key)
            && other.key != old_key
        {
            return Err(format!("{online}'s key is already registered as '{}'", other.nickname));
        }
        if let Some(r) = self.registry.users.iter_mut().find(|r| r.key == old_key) {
            r.key = new_key.clone();
        }
        self.registry.save();
        // Both: whoever is online with the old key loses the registration.
        self.refresh_user(&old_key);
        self.refresh_user(&new_key);
        Ok(format!(
            "'{registered}' is now bound to {online}'s key {}. They should connect as '{registered}' from now on",
            short_key(&new_key)
        ))
    }
}

/// One-time admin setup code: "XXXX-XXXX" from an alphabet without
/// look-alike characters.
pub(super) fn new_setup_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut rng = rand::thread_rng();
    let mut s: String = (0..8).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect();
    s.insert(4, '-');
    s
}

/// Codes are compared without dashes and spaces, case-insensitively.
pub(super) fn normalize_code(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
}
