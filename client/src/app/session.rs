//! Connection state: the session with a server, chat lines, private conversations.

use super::*;

pub(super) enum ChatLine {
    Msg { author: String, text: String, ts: i64 },
    System { text: String, ts: i64 },
}

pub(super) struct PmConversation {
    pub(super) name: String,
    pub(super) lines: Vec<ChatLine>,
    pub(super) unread: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum ChatTarget {
    Channel,
    Private(UserId),
}

pub(super) struct Session {
    pub(super) my_id: UserId,
    pub(super) server_name: String,
    pub(super) motd: String,
    pub(super) motd_shown: bool,
    pub(super) channels: Vec<ChannelInfo>,
    pub(super) users: BTreeMap<UserId, UserInfo>,
    pub(super) channel_chat: HashMap<ChannelId, Vec<ChatLine>>,
    pub(super) pms: BTreeMap<UserId, PmConversation>,
    pub(super) channel_unread: bool,
    /// The server has no owner yet and accepts the setup code.
    pub(super) admin_setup: bool,
    /// Server version from `Welcome`; empty if the server did not send it.
    pub(super) server_version: String,
    /// Optional features the server supports (`shared::FEATURES` names).
    pub(super) server_features: Vec<String>,
    /// Connection quality, from `NetEvent::Link`.
    pub(super) link: super::link::LinkMonitor,
    /// Protocol major agreed with the server.
    pub(super) protocol: Option<u32>,
    /// The server's roles, most senior first.
    pub(super) roles: shared::Roles,
    /// Since when each user watches what they watch now (seen by this client:
    /// those already watching on connect count from then).
    pub(super) watch_since: HashMap<UserId, Instant>,
    /// A viewer joined my share: the counter is tinted until 2 s after.
    pub(super) viewer_flash: Option<Instant>,
    /// Last viewer sound per viewer, and at key 0 the last one at all:
    /// someone flipping the stream on and off must not make a din.
    pub(super) viewer_sounds: HashMap<UserId, Instant>,
}

/// One viewer may make a sound at most this often...
const VIEWER_SOUND_EACH: Duration = Duration::from_secs(4);
/// ...and viewers together this often (several joining at once: one sound).
const VIEWER_SOUND_ANY: Duration = Duration::from_millis(600);

impl Session {
    /// Whether the server supports an optional feature (both sides must).
    #[allow(dead_code)] // No optional features yet; see `shared::FEATURES`.
    pub(super) fn server_has(&self, feature: &str) -> bool {
        self.server_features.iter().any(|f| f == feature)
    }
}

impl Session {
    pub(super) fn me(&self) -> Option<&UserInfo> {
        self.users.get(&self.my_id)
    }

    pub(super) fn my_role(&self) -> Option<shared::RoleId> {
        self.me().and_then(|m| m.role)
    }

    /// My rights, by my role. Only for what to show: the server checks.
    pub(super) fn my_perms(&self) -> shared::Perms {
        self.roles.perms(self.my_role())
    }

    pub(super) fn my_channel(&self) -> Option<ChannelId> {
        self.me().map(|u| u.channel_id)
    }

    /// Whether `id` is sharing the screen in their channel.
    pub(super) fn is_live(&self, id: UserId) -> bool {
        self.users.get(&id).is_some_and(|u| u.streams.contains(&MediaKind::Screen))
    }

    /// Viewers of `streamer`'s share, newest first.
    pub(super) fn viewers_of(&self, streamer: UserId) -> Vec<&UserInfo> {
        let Some(s) = self.users.get(&streamer).filter(|_| self.is_live(streamer)) else { return Vec::new() };
        let mut v: Vec<&UserInfo> =
            self.users.values().filter(|u| u.watching == Some(streamer) && u.channel_id == s.channel_id).collect();
        let since = |u: &UserInfo| self.watch_since.get(&u.id).copied();
        v.sort_by(|a, b| {
            since(b).cmp(&since(a)).then_with(|| a.nickname.to_lowercase().cmp(&b.nickname.to_lowercase()))
        });
        v
    }

    /// Whether a viewer sound for `viewer` may play now; notes it if so.
    pub(super) fn viewer_sound_ok(&mut self, viewer: UserId) -> bool {
        let recent = |k: UserId, d: Duration| self.viewer_sounds.get(&k).is_some_and(|t| t.elapsed() < d);
        if recent(viewer, VIEWER_SOUND_EACH) || recent(0, VIEWER_SOUND_ANY) {
            return false;
        }
        let now = Instant::now();
        self.viewer_sounds.insert(viewer, now);
        self.viewer_sounds.insert(0, now);
        true
    }

    pub(super) fn channel_name(&self, id: ChannelId) -> String {
        self.channels.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default()
    }

    pub(super) fn users_in(&self, ch: ChannelId) -> Vec<&UserInfo> {
        let mut v: Vec<_> = self.users.values().filter(|u| u.channel_id == ch).collect();
        v.sort_by_key(|u| u.nickname.to_lowercase());
        v
    }

    /// System line in the current channel's chat.
    pub(super) fn system_line(&mut self, text: String) {
        if let Some(ch) = self.my_channel() {
            self.channel_chat.entry(ch).or_default().push(ChatLine::System { text, ts: shared::unix_now() });
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum ConnState {
    Idle,
    Connecting,
    Reconnecting(u32),
    Connected,
}

/// Client copy of the server's chat limit (`CHAT_BURST`, `CHAT_PER_SEC` in
/// server/src/conn.rs): a burst of 10 messages, then 1 per second. Keep in sync.
pub(super) struct ChatLimit {
    pub(super) tokens: f64,
    pub(super) last: Instant,
}

impl ChatLimit {
    const BURST: f64 = 10.0;
    const PER_SEC: f64 = 1.0;

    pub(super) fn new() -> Self {
        Self { tokens: Self::BURST, last: Instant::now() }
    }

    pub(super) fn try_take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * Self::PER_SEC).min(Self::BURST);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
