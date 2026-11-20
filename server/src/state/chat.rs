//! Chat: channel messages with their history, private messages.

use super::*;

impl State {
    /// A message to the sender's channel: kept in the history and sent to
    /// everyone in the channel.
    pub(super) fn chat_send(&mut self, me: &UserInfo, channel_id: ChannelId, text: &str) -> Result<(), ErrorCode> {
        let text = text.trim();
        if text.is_empty() || channel_id != me.channel_id {
            return Ok(());
        }
        check_length(text)?;
        let msg = ChatMessage {
            id: self.next_msg_id,
            channel_id,
            author_id: me.id,
            author_name: me.nickname.clone(),
            text: text.to_string(),
            timestamp: shared::unix_now(),
        };
        self.next_msg_id += 1;
        let hist = self.history.entry(channel_id).or_default();
        hist.push_back(msg.clone());
        while hist.len() > self.config.history_size {
            hist.pop_front();
        }
        let out = ServerMsg::Chat(msg);
        for u in self.users.values().filter(|u| u.info.channel_id == channel_id) {
            u.push(out.clone());
        }
        Ok(())
    }

    /// A private message: both sides get it, the sender as confirmation.
    pub(super) fn private_send(&mut self, me: &UserInfo, to: UserId, text: &str) -> Result<(), ErrorCode> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        check_length(text)?;
        if to == me.id {
            return Err(ErrorCode::CannotTargetSelf);
        }
        let target = self.users.get(&to).ok_or(ErrorCode::NoSuchUser)?;
        let pm = PrivateMessage {
            from_id: me.id,
            from_name: me.nickname.clone(),
            to_id: to,
            to_name: target.info.nickname.clone(),
            text: text.to_string(),
            timestamp: shared::unix_now(),
        };
        self.send(to, ServerMsg::Private(pm.clone()));
        self.send(me.id, ServerMsg::Private(pm));
        Ok(())
    }

    pub(super) fn send_history(&self, user: UserId, channel_id: ChannelId) {
        let messages = self.history.get(&channel_id).map(|h| h.iter().cloned().collect()).unwrap_or_default();
        self.send(user, ServerMsg::ChannelHistory { channel_id, messages });
    }
}

/// Refuses a message longer than `shared::MAX_MESSAGE_CHARS`.
fn check_length(text: &str) -> Result<(), ErrorCode> {
    if text.chars().count() > shared::MAX_MESSAGE_CHARS {
        return Err(ErrorCode::MessageTooLong);
    }
    Ok(())
}
