//! Channels: lookup, limits, create/edit/delete, moving users between them.

use super::*;

impl State {
    pub(super) fn channel(&self, id: ChannelId) -> Option<&ChannelInfo> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn channel_name(&self, id: ChannelId) -> String {
        self.channel(id).map(|c| c.name.clone()).unwrap_or_else(|| format!("#{id}"))
    }

    pub(super) fn channel_count(&self, id: ChannelId) -> usize {
        self.users.values().filter(|u| u.info.channel_id == id).count()
    }

    pub(super) fn channel_has_room(&self, ch: &ChannelInfo) -> bool {
        ch.max_users.is_none_or(|m| self.channel_count(ch.id) < m as usize)
    }

    /// Normalizes a channel's name, description and limit for storage and
    /// validates them. `except` is the channel currently being renamed.
    pub(super) fn check_channel(
        &self,
        name: &str,
        description: &str,
        max_users: Option<u32>,
        except: Option<ChannelId>,
    ) -> Result<(String, String, Option<u32>), ErrorCode> {
        let name = name.trim().to_string();
        let description = description.trim().to_string();
        if !shared::validate_channel_name(&name)
            || description.chars().count() > shared::MAX_CHANNEL_DESCRIPTION_CHARS
            || description.chars().any(|c| c.is_control())
        {
            return Err(ErrorCode::InvalidChannelName);
        }
        let lower = name.to_lowercase();
        if self.channels.iter().any(|c| Some(c.id) != except && c.name.to_lowercase() == lower) {
            return Err(ErrorCode::ChannelNameTaken);
        }
        // A limit of 0 means "no limit".
        Ok((name, description, max_users.filter(|m| *m > 0)))
    }

    pub fn find_channel(&self, query: &str) -> Option<ChannelId> {
        if let Ok(id) = query.parse::<ChannelId>()
            && self.channel(id).is_some()
        {
            return Some(id);
        }
        let q = query.to_lowercase();
        self.channels.iter().find(|c| c.name.to_lowercase() == q).map(|c| c.id)
    }

    pub fn create_channel(
        &mut self,
        name: &str,
        description: &str,
        max_users: Option<u32>,
        by: &str,
    ) -> Result<ChannelInfo, ErrorCode> {
        let (name, description, max_users) = self.check_channel(name, description, max_users, None)?;
        let ch = ChannelInfo { id: self.next_channel_id, name, description, max_users };
        self.next_channel_id += 1;
        info!("{by}: created channel '{}', {}", ch.name, limit_text(ch.max_users));
        self.channels.push(ch.clone());
        self.broadcast(&ServerMsg::ChannelAdded(ch.clone()), None);
        self.save_channels();
        Ok(ch)
    }

    pub fn edit_channel(
        &mut self,
        id: ChannelId,
        name: &str,
        description: &str,
        max_users: Option<u32>,
        by: &str,
    ) -> Result<ChannelInfo, ErrorCode> {
        let old = self.channel(id).cloned().ok_or(ErrorCode::NoSuchChannel)?;
        let (name, description, max_users) = self.check_channel(name, description, max_users, Some(id))?;
        let ch = ChannelInfo { id, name, description, max_users };
        if ch == old {
            return Ok(ch);
        }
        if ch.name != old.name {
            info!("{by}: channel '{}' renamed to '{}'", old.name, ch.name);
        }
        if ch.max_users != old.max_users {
            info!("{by}: channel '{}', {}", ch.name, limit_text(ch.max_users));
        }
        if let Some(c) = self.channels.iter_mut().find(|c| c.id == id) {
            *c = ch.clone();
        }
        self.broadcast(&ServerMsg::ChannelUpdated(ch.clone()), None);
        self.save_channels();
        Ok(ch)
    }

    /// Deletes a channel. Its members go to the first channel with space,
    /// or, if none has space, to the first channel over its limit.
    pub fn delete_channel(&mut self, id: ChannelId, by: &str) -> Result<ChannelInfo, ErrorCode> {
        let pos = self.channels.iter().position(|c| c.id == id).ok_or(ErrorCode::NoSuchChannel)?;
        if self.channels.len() == 1 {
            return Err(ErrorCode::LastChannel);
        }
        let removed = self.channels.remove(pos);
        self.history.remove(&id);
        let mut members: Vec<UserId> =
            self.users.values().filter(|u| u.info.channel_id == id).map(|u| u.info.id).collect();
        members.sort_unstable();
        info!("{by}: deleted channel '{}', users moved: {}", removed.name, members.len());
        for uid in members {
            let target = self.channels.iter().find(|c| self.channel_has_room(c)).unwrap_or(&self.channels[0]).id;
            self.move_user(uid, target);
        }
        self.broadcast(&ServerMsg::ChannelRemoved { channel_id: id }, None);
        self.save_channels();
        Ok(removed)
    }

    pub(super) fn save_channels(&mut self) {
        self.config.save_channels(&self.channels);
    }

    /// A user asks to move to another channel, which must have room.
    pub(super) fn join_channel(&mut self, me: &UserInfo, channel_id: ChannelId) -> Result<(), ErrorCode> {
        let ch = self.channel(channel_id).ok_or(ErrorCode::NoSuchChannel)?;
        if me.channel_id == channel_id {
            return Ok(());
        }
        if !self.channel_has_room(ch) {
            return Err(ErrorCode::ChannelFull);
        }
        self.move_user(me.id, channel_id);
        Ok(())
    }

    /// Moves a user to another channel without checking the limit.
    /// Sharing and watching are scoped to the channel,
    /// so both stop on switching.
    pub(super) fn move_user(&mut self, id: UserId, channel_id: ChannelId) {
        let Some(u) = self.users.get(&id) else { return };
        let was_sharing = u.info.streams.contains(&MediaKind::Screen);
        if was_sharing {
            info!("{}: screen share stopped on channel switch", u.info.nickname);
        }
        self.update_user(id, |u| {
            u.channel_id = channel_id;
            u.streams.retain(|k| *k != MediaKind::Screen);
            u.watching = None;
        });
        if was_sharing {
            self.drop_viewers(id);
        }
        self.send_history(id, channel_id);
    }
}

pub(super) fn limit_text(max_users: Option<u32>) -> String {
    match max_users {
        Some(m) => format!("limit {m}"),
        None => "no limit".into(),
    }
}
