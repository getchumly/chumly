//! Voice state, screen shares and who watches them, and where the media
//! datagrams from a user go.

use super::*;

impl State {
    pub(super) fn set_voice_state(&mut self, id: UserId, muted: bool, deafened: bool) -> Result<(), ErrorCode> {
        self.update_user(id, |u| {
            u.muted = muted || deafened;
            u.deafened = deafened;
        });
        Ok(())
    }

    /// Starts or stops the user's stream of `kind`. Only the screen share is
    /// switched: voice is always on, and the screen share audio goes
    /// together with the video.
    pub(super) fn set_streaming(&mut self, me: &UserInfo, kind: MediaKind, on: bool) -> Result<(), ErrorCode> {
        if kind != MediaKind::Screen {
            return Err(ErrorCode::Unsupported);
        }
        let sharing = me.streams.contains(&MediaKind::Screen);
        if on && !sharing {
            info!("{}: started screen share", me.nickname);
            self.update_user(me.id, |u| u.streams.push(MediaKind::Screen));
        } else if !on && sharing {
            info!("{}: stopped screen share", me.nickname);
            self.update_user(me.id, |u| u.streams.retain(|k| *k != MediaKind::Screen));
            self.drop_viewers(me.id);
        }
        Ok(())
    }

    /// Starts watching `target`'s screen share (someone sharing in the same
    /// channel), or stops watching with `None`.
    pub(super) fn watch(&mut self, me: &UserInfo, target: Option<UserId>) -> Result<(), ErrorCode> {
        let Some(target) = target else {
            if me.watching.is_some() {
                self.update_user(me.id, |u| u.watching = None);
            }
            return Ok(());
        };
        if !self.users.get(&me.id).is_some_and(|u| u.can_view) {
            return Err(ErrorCode::Unsupported);
        }
        if target == me.id {
            return Err(ErrorCode::CannotTargetSelf);
        }
        let streaming = self
            .users
            .get(&target)
            .is_some_and(|t| t.info.channel_id == me.channel_id && t.info.streams.contains(&MediaKind::Screen));
        if !streaming {
            return Err(ErrorCode::NotStreaming);
        }
        if me.watching != Some(target) {
            self.update_user(me.id, |u| u.watching = Some(target));
        }
        // A new viewer needs a keyframe, otherwise they see garbage.
        self.send(target, ServerMsg::KeyframeRequest);
        Ok(())
    }

    /// A viewer lost frames: passes the request on to the streamer they watch.
    pub(super) fn request_keyframe(&self, me: &UserInfo, streamer: UserId) -> Result<(), ErrorCode> {
        if me.watching == Some(streamer) {
            self.send(streamer, ServerMsg::KeyframeRequest);
        }
        Ok(())
    }

    /// Everyone watching `streamer`'s share stops: the share ended, moved
    /// away or its owner left. Clients see it in `UserInfo::watching`.
    pub(super) fn drop_viewers(&mut self, streamer: UserId) {
        let viewers: Vec<UserId> =
            self.users.values().filter(|u| u.info.watching == Some(streamer)).map(|u| u.info.id).collect();
        for v in viewers {
            self.update_user(v, |u| u.watching = None);
        }
    }

    /// Connections of viewers that should receive `from`'s screen share.
    pub fn screen_targets(&self, from: UserId) -> Vec<quinn::Connection> {
        let Some(me) = self.users.get(&from) else { return Vec::new() };
        if !me.info.streams.contains(&MediaKind::Screen) {
            return Vec::new();
        }
        self.users
            .values()
            .filter(|u| u.info.watching == Some(from) && u.can_view && u.info.channel_id == me.info.channel_id)
            .map(|u| u.conn.clone())
            .collect()
    }

    /// Connections that should receive voice from user `from`.
    pub fn voice_targets(&self, from: UserId) -> Vec<quinn::Connection> {
        let Some(me) = self.users.get(&from) else { return Vec::new() };
        if me.info.muted {
            return Vec::new();
        }
        self.users
            .values()
            .filter(|u| u.info.id != from && u.info.channel_id == me.info.channel_id && !u.info.deafened)
            .map(|u| u.conn.clone())
            .collect()
    }
}
