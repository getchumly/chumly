//! Dispatch of client messages: the rights check, then one handler per
//! message. A handler returns `Err(code)` to answer the sender with an error.

use super::*;

impl State {
    pub fn handle(&mut self, id: UserId, msg: ClientMsg) -> After {
        let Some(me) = self.users.get(&id).map(|u| u.info.clone()) else {
            return After::Nothing;
        };
        // Rights are checked here, on the server: the client hiding the
        // buttons is not a protection. Rank against the target is checked
        // by each action.
        let owner_only = matches!(msg, ClientMsg::GetServerSettings | ClientMsg::SetPassword { .. });
        if !allowed(self.registry.roles.perms(me.role), &msg) || (owner_only && me.role != Some(shared::OWNER_ROLE)) {
            info!("{}: request refused, no right for it", me.nickname);
            self.send_error(id, ErrorCode::NotAdmin);
            return After::Nothing;
        }
        let my_key = self.users.get(&id).and_then(|u| u.key.clone());
        let actor = Actor { role: me.role, key: my_key.as_deref() };
        let by = me.nickname.as_str();
        let result = match msg {
            ClientMsg::Hello { .. } => Ok(()),
            ClientMsg::AdminSetup { code } => return self.admin_setup(&me, &code),

            // Channels and chat.
            ClientMsg::JoinChannel { channel_id } => self.join_channel(&me, channel_id),
            ClientMsg::ChatSend { channel_id, text } => self.chat_send(&me, channel_id, &text),
            ClientMsg::PrivateSend { to, text } => self.private_send(&me, to, &text),

            // Voice and screen share.
            ClientMsg::SetVoiceState { muted, deafened } => self.set_voice_state(id, muted, deafened),
            ClientMsg::StartStream { kind } => self.set_streaming(&me, kind, true),
            ClientMsg::StopStream { kind } => self.set_streaming(&me, kind, false),
            ClientMsg::WatchStream { user_id } => self.watch(&me, user_id),
            ClientMsg::RequestKeyframe { user_id } => self.request_keyframe(&me, user_id),

            // Moderation.
            ClientMsg::Kick { user_id, reason } => self.moderate(&me, user_id, reason, Punish::Kick),
            ClientMsg::Ban { user_id, reason, duration_secs } => {
                self.moderate(&me, user_id, reason, Punish::Ban { secs: duration_secs })
            }
            ClientMsg::ListBans => {
                self.send_bans(id);
                Ok(())
            }
            ClientMsg::Unban { ban } => {
                let result = self.unban(&ban, by);
                self.send_bans(id);
                result
            }

            // The panel's users tab gets the fresh list after every change,
            // also after a refused one.
            ClientMsg::ListUsers => {
                self.send_panel_users(id);
                Ok(())
            }
            ClientMsg::SetRole { user_id, role } => {
                let result = self.set_role(user_id, role, actor, by);
                self.send_panel_users(id);
                result
            }
            ClientMsg::SetRoleByKey { key, role } => {
                let result = self.set_role_key(&key, role, actor, by);
                self.send_panel_users(id);
                result
            }
            ClientMsg::SetRegistered { user_id, registered } => {
                let result = self.set_registered(user_id, registered, actor, by);
                self.send_panel_users(id);
                result
            }
            ClientMsg::UnregisterKey { key } => {
                let result = self.unregister_key(&key, actor, by);
                self.send_panel_users(id);
                result
            }

            // Channel and role editing.
            ClientMsg::CreateChannel { name, description, max_users } => {
                self.create_channel(&name, &description, max_users, by).map(drop)
            }
            ClientMsg::EditChannel { channel_id, name, description, max_users } => {
                self.edit_channel(channel_id, &name, &description, max_users, by).map(drop)
            }
            ClientMsg::DeleteChannel { channel_id } => self.delete_channel(channel_id, by).map(drop),
            ClientMsg::CreateRole { role } => self.create_role(role, actor, by).map(drop),
            ClientMsg::EditRole { role } => self.edit_role(role, actor, by),
            ClientMsg::DeleteRole { role_id } => self.delete_role(role_id, actor, by),
            ClientMsg::MoveRole { role_id, index } => self.move_role(role_id, index as usize, actor, by),

            // Server settings, owner only (checked above).
            ClientMsg::GetServerSettings => {
                self.send_server_settings(id);
                Ok(())
            }
            ClientMsg::SetPassword { password } => {
                let result = self.set_password(&password, by);
                self.send_server_settings(id);
                result
            }
        };
        if let Err(code) = result {
            self.send_error(id, code);
        }
        After::Nothing
    }
}

/// Whether a user with `perms` may send `msg` at all.
fn allowed(perms: Perms, msg: &ClientMsg) -> bool {
    match msg {
        ClientMsg::ListUsers => perms.intersects(Perms::PANEL),
        ClientMsg::Kick { .. } => perms.contains(Perms::KICK),
        ClientMsg::Ban { .. } | ClientMsg::ListBans | ClientMsg::Unban { .. } => perms.contains(Perms::BAN),
        ClientMsg::SetRole { .. } | ClientMsg::SetRoleByKey { .. } => perms.contains(Perms::ASSIGN_ROLES),
        ClientMsg::SetRegistered { .. } | ClientMsg::UnregisterKey { .. } => perms.contains(Perms::REGISTER),
        ClientMsg::CreateChannel { .. } | ClientMsg::EditChannel { .. } | ClientMsg::DeleteChannel { .. } => {
            perms.contains(Perms::CHANNELS)
        }
        ClientMsg::CreateRole { .. }
        | ClientMsg::EditRole { .. }
        | ClientMsg::DeleteRole { .. }
        | ClientMsg::MoveRole { .. } => perms.contains(Perms::EDIT_ROLES),
        // Owner only: no right gives it, the role is checked in `handle`.
        ClientMsg::GetServerSettings | ClientMsg::SetPassword { .. } => perms == Perms::ALL,
        ClientMsg::Hello { .. }
        | ClientMsg::AdminSetup { .. }
        | ClientMsg::JoinChannel { .. }
        | ClientMsg::ChatSend { .. }
        | ClientMsg::PrivateSend { .. }
        | ClientMsg::SetVoiceState { .. }
        | ClientMsg::StartStream { .. }
        | ClientMsg::StopStream { .. }
        | ClientMsg::WatchStream { .. }
        | ClientMsg::RequestKeyframe { .. } => true,
    }
}
