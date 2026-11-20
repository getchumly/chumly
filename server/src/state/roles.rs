//! Editing the role list: create, edit, delete, move. Every change is saved
//! to users.toml and sent to everyone (`ServerMsg::Roles`).
//!
//! Rules (besides the `EDIT_ROLES` right, checked in `handle`): only roles
//! below one's own (`Roles::can_edit`), new roles go to the bottom, a move
//! stays below one's own role, and only rights the editor has can be turned
//! on or off.

use shared::{MAX_ROLES, OWNER_ICON, RoleId, RoleInfo};

use super::*;

impl State {
    fn roles_changed(&mut self) {
        self.registry.save();
        self.broadcast(&ServerMsg::Roles(self.registry.roles.clone()), None);
    }

    /// Name, color and icon checks; the name must be free (other than `own`).
    /// The crown is the owner's: another role with it would pass for one.
    fn check_role_fields(&self, role: &RoleInfo, own: Option<RoleId>) -> Result<(), ErrorCode> {
        let style_ok = |s: &str| s.is_empty() || shared::validate_style_name(s);
        let crown = role.icon == OWNER_ICON;
        if !shared::validate_role_name(&role.name) || !style_ok(&role.color) || !style_ok(&role.icon) || crown {
            return Err(ErrorCode::InvalidRoleName);
        }
        let taken = self.registry.role_by_name(&role.name).is_some_and(|r| Some(r.id) != own);
        if taken {
            return Err(ErrorCode::RoleNameTaken);
        }
        Ok(())
    }

    /// Rights that may be switched: only those the actor has.
    fn check_perm_change(&self, actor: Actor<'_>, old: Perms, new: Perms) -> Result<(), ErrorCode> {
        let mine = self.registry.roles.perms(actor.role);
        let changed = Perms::from_bits(old.bits() ^ new.bits());
        if mine.contains(changed) { Ok(()) } else { Err(ErrorCode::NotAdmin) }
    }

    pub fn create_role(&mut self, role: RoleInfo, actor: Actor<'_>, by: &str) -> Result<RoleId, ErrorCode> {
        if self.registry.roles.list.len() >= MAX_ROLES {
            return Err(ErrorCode::TooManyRoles);
        }
        self.check_role_fields(&role, None)?;
        self.check_perm_change(actor, Perms::NONE, role.perms)?;
        let name = role.name.clone();
        let id = self.registry.add_role(role);
        info!("{by}: created role {name} ({id})");
        self.roles_changed();
        Ok(id)
    }

    pub fn edit_role(&mut self, role: RoleInfo, actor: Actor<'_>, by: &str) -> Result<(), ErrorCode> {
        let old = self.registry.roles.get(role.id).cloned().ok_or(ErrorCode::NoSuchRole)?;
        if !self.registry.roles.can_edit(actor.role, role.id) {
            return Err(ErrorCode::NotAdmin);
        }
        self.check_role_fields(&role, Some(role.id))?;
        self.check_perm_change(actor, old.perms, role.perms)?;
        info!("{by}: edited role {} ({}): rights {}", role.name, role.id, role.perms.names().join(", "));
        let i = self.registry.roles.index(role.id).expect("found above");
        self.registry.roles.list[i] = role;
        self.roles_changed();
        Ok(())
    }

    pub fn delete_role(&mut self, id: RoleId, actor: Actor<'_>, by: &str) -> Result<(), ErrorCode> {
        let name = self.registry.roles.get(id).map(|r| r.name.clone()).ok_or(ErrorCode::NoSuchRole)?;
        if !self.registry.roles.can_edit(actor.role, id) {
            return Err(ErrorCode::NotAdmin);
        }
        let keys = self.registry.remove_role(id);
        info!("{by}: deleted role {name} ({id}), taken from {} users", keys.len());
        // The list first: clients then get users without the role.
        self.roles_changed();
        for k in &keys {
            self.refresh_user(k);
        }
        Ok(())
    }

    /// Moves a role to `index`; it must stay below the actor's role.
    pub fn move_role(&mut self, id: RoleId, index: usize, actor: Actor<'_>, by: &str) -> Result<(), ErrorCode> {
        if self.registry.roles.get(id).is_none() {
            return Err(ErrorCode::NoSuchRole);
        }
        if !self.registry.roles.can_edit(actor.role, id) {
            return Err(ErrorCode::NotAdmin);
        }
        // The actor's own role stays where it is, so "below" is by its
        // place in the list without the moved role.
        let mine = actor.role.and_then(|r| self.registry.roles.index(r)).ok_or(ErrorCode::NotAdmin)?;
        if index <= mine {
            return Err(ErrorCode::NotAdmin);
        }
        self.registry.move_role(id, index);
        let name = self.registry.roles.get(id).map(|r| r.name.clone()).unwrap_or_default();
        info!("{by}: moved role {name} ({id}) to place {}", self.registry.roles.index(id).unwrap_or(0) + 1);
        self.roles_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use shared::{OWNER_ROLE, Roles};

    use super::*;
    use crate::bans::BanList;
    use crate::config::ServerConfig;
    use crate::users::Registry;

    fn state() -> State {
        let mut registry = Registry::default();
        registry.roles = Roles::defaults();
        State::new(ServerConfig::default(), BanList::default(), registry)
    }

    fn role(name: &str, perms: Perms) -> RoleInfo {
        RoleInfo { id: 0, name: name.into(), color: "teal".into(), icon: "heart".into(), perms }
    }

    const ADMIN: Actor<'static> = Actor { role: Some(2), key: None };

    #[test]
    fn editing_follows_the_hierarchy() {
        let mut st = state();
        // Created at the bottom; names are unique and checked.
        let helper = st.create_role(role("Helper", Perms::KICK), ADMIN, "a").unwrap();
        assert_eq!(st.registry.roles.list.last().unwrap().id, helper);
        assert_eq!(st.create_role(role("helper", Perms::NONE), ADMIN, "a"), Err(ErrorCode::RoleNameTaken));
        assert_eq!(st.create_role(role(" x", Perms::NONE), ADMIN, "a"), Err(ErrorCode::InvalidRoleName));
        let mut bad = role("Y", Perms::NONE);
        bad.color = "Not A Color".into();
        assert_eq!(st.create_role(bad, ADMIN, "a"), Err(ErrorCode::InvalidRoleName));
        let mut crown = role("Z", Perms::NONE);
        crown.icon = OWNER_ICON.into();
        assert_eq!(st.create_role(crown, Actor::CONSOLE, "c"), Err(ErrorCode::InvalidRoleName));

        // Only roles below one's own: the admin cannot touch the owner or itself.
        let mut own = st.registry.roles.get(2).unwrap().clone();
        own.name = "Boss".into();
        assert_eq!(st.edit_role(own, ADMIN, "a"), Err(ErrorCode::NotAdmin));
        assert_eq!(st.delete_role(OWNER_ROLE, Actor::CONSOLE, "c"), Err(ErrorCode::NotAdmin));
        // Moves stay below one's own role.
        assert_eq!(st.move_role(helper, 1, ADMIN, "a"), Err(ErrorCode::NotAdmin));
        st.move_role(helper, 2, ADMIN, "a").unwrap();
        assert_eq!(st.registry.roles.index(helper), Some(2));
        assert_eq!(st.registry.roles.index(3), Some(3));

        // Deleting takes the role from its users.
        st.registry.register("K", "v").role = Some(helper);
        st.delete_role(helper, ADMIN, "a").unwrap();
        assert_eq!(st.registry.by_key("K").unwrap().role, None);
        assert_eq!(st.delete_role(helper, ADMIN, "a"), Err(ErrorCode::NoSuchRole));
    }

    #[test]
    fn only_own_rights_can_be_switched() {
        let mut st = state();
        // An admin without the ban right.
        st.registry.roles.list[1].perms = Perms::EDIT_ROLES | Perms::KICK;
        assert_eq!(st.create_role(role("Banner", Perms::BAN), ADMIN, "a"), Err(ErrorCode::NotAdmin));
        let kicker = st.create_role(role("Kicker", Perms::KICK), ADMIN, "a").unwrap();
        // A right set by the owner stays when the admin edits other fields...
        let mut r = st.registry.roles.get(kicker).unwrap().clone();
        r.perms = Perms::KICK | Perms::BAN;
        st.edit_role(r.clone(), Actor::CONSOLE, "c").unwrap();
        r.name = "Kicker 2".into();
        st.edit_role(r.clone(), ADMIN, "a").unwrap();
        assert_eq!(st.registry.roles.get(kicker).unwrap().perms, Perms::KICK | Perms::BAN);
        // ...but the admin cannot take it away either.
        r.perms = Perms::KICK;
        assert_eq!(st.edit_role(r, ADMIN, "a"), Err(ErrorCode::NotAdmin));
    }
}
