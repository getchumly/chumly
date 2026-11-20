//! Control messages. Sent over a reliable bidirectional QUIC stream,
//! one stream per connection. These are the types the application uses;
//! the wire format (protobuf) and its compatibility rules are in `wire.rs`.

use crate::media::MediaKind;

/// Application name used for windows, logs and the config directory.
pub const APP_NAME: &str = "Chumly";
/// This build's version, sent in `Hello` / `Welcome` for display and logs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Major protocol versions this build speaks, newest first. A major version
/// is an incompatible change; everything compatible goes through protobuf's
/// rules (see `wire.rs`) and needs no version. The major is agreed during the
/// TLS handshake via ALPN (`chumly/<major>`): client and server list theirs and
/// the newest common one wins; with none in common the handshake fails with
/// `no_application_protocol`. Policy: keep the previous major too for a while.
pub const PROTOCOL_MAJORS: &[u32] = &[1];
/// An optional feature, exchanged by name in `Hello` / `Welcome`.
///
/// Compatibility is judged by features, never by version numbers: a release
/// may change only the client (interface) or only the server (security,
/// limits), and the other side need not care. A side uses a feature with a
/// peer only if the peer listed it; otherwise it works as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    /// Stable name on the wire; never renamed.
    pub name: &'static str,
    /// The client can use it only with a server that supports it. A server
    /// without it gets a "the server does not support ..." note for its owner.
    pub needs_server: bool,
}

/// Optional features this build supports. Empty so far; a new optional
/// feature adds an entry here and a label in the client's `i18n.rs`.
pub const FEATURES: &[Feature] = &[];

/// Names of `FEATURES`, as sent in `Hello` / `Welcome`.
pub fn feature_names() -> Vec<String> {
    FEATURES.iter().map(|f| f.name.to_string()).collect()
}

/// Features of `ours` that need the server but are missing from the list the
/// server sent: what the owner should be told the server lacks.
pub fn missing_on_server<'a>(ours: &'a [Feature], server: &[String]) -> Vec<&'a Feature> {
    ours.iter().filter(|f| f.needs_server && !server.iter().any(|s| s == f.name)).collect()
}

/// ALPN name of the status query: a separate connection, before and without
/// logging in, answered with one `ServerStatus` on a unidirectional stream.
/// A server with the public status turned off does not offer it; the client
/// then knows the server is up (the handshake got that far) but not the counts.
pub const STATUS_ALPN: &[u8] = b"chumly-status/1";

/// What anyone may learn about a server without logging in. Counts only, no
/// nicknames: a stranger cannot find out who is on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub version: String,
    pub users_online: u32,
    pub max_users: u32,
    pub features: Vec<String>,
}

/// ALPN protocol names for `PROTOCOL_MAJORS`, newest first.
pub fn alpn_protocols() -> Vec<Vec<u8>> {
    PROTOCOL_MAJORS.iter().map(|m| format!("chumly/{m}").into_bytes()).collect()
}

/// The major version from a negotiated ALPN name such as `chumly/1`.
pub fn major_from_alpn(alpn: &[u8]) -> Option<u32> {
    std::str::from_utf8(alpn).ok()?.strip_prefix("chumly/")?.parse().ok()
}
/// Server name the client uses for TLS. The certificate is verified
/// by fingerprint, so a real domain name is not needed.
pub const TLS_SERVER_NAME: &str = "chumly";
/// Name in the self-signed client certificate. Servers ignore it and
/// identify the user by the certificate's public key.
pub const TLS_CLIENT_NAME: &str = "chumly-client";
pub const DEFAULT_PORT: u16 = 7777;
pub const MAX_NICKNAME_CHARS: usize = 32;
pub const MAX_MESSAGE_CHARS: usize = 2000;
pub const MAX_CHANNEL_NAME_CHARS: usize = 32;
pub const MAX_CHANNEL_DESCRIPTION_CHARS: usize = 200;

pub type UserId = u32;
pub type ChannelId = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: ChannelId,
    pub name: String,
    pub description: String,
    pub max_users: Option<u32>,
}

/// A server role, by id. Ids are not reused while the role list exists, so
/// a stale id (a deleted role) just finds nothing.
pub type RoleId = u32;
/// The owner role: always first in the list, all rights, never edited,
/// given or taken from a client; it moves only by the console command `owner`.
pub const OWNER_ROLE: RoleId = 1;
/// The owner's icon; no other role may take it.
pub const OWNER_ICON: &str = "crown";
pub const MAX_ROLES: usize = 50;
pub const MAX_ROLE_NAME_CHARS: usize = 32;
pub const MAX_PASSWORD_CHARS: usize = 64;

/// A new server password: up to `MAX_PASSWORD_CHARS`, no control characters,
/// no spaces at the ends (they would be lost when typed or pasted). Empty
/// means "no password" and is valid.
pub fn validate_password(password: &str) -> bool {
    password.chars().count() <= MAX_PASSWORD_CHARS
        && !password.chars().any(char::is_control)
        && password.trim() == password
}

/// Rights of a role, a bit set. Bits this version does not know are dropped
/// on reading: rights are always checked by the server, the client only
/// decides what to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Perms(u32);

impl Perms {
    pub const NONE: Self = Self(0);
    /// Kick users below.
    pub const KICK: Self = Self(1);
    /// Ban users below; see and lift bans.
    pub const BAN: Self = Self(1 << 1);
    /// Create, edit and delete channels.
    pub const CHANNELS: Self = Self(1 << 2);
    /// Register nicknames to keys and remove registrations (of users below).
    pub const REGISTER: Self = Self(1 << 3);
    /// Give and take roles below one's own.
    pub const ASSIGN_ROLES: Self = Self(1 << 4);
    /// Create, edit, reorder and delete roles below one's own.
    pub const EDIT_ROLES: Self = Self(1 << 5);
    pub const ALL: Self = Self((1 << 6) - 1);
    /// Rights the admin panel serves: with any of them it is shown.
    pub const PANEL: Self = Self(Self::BAN.0 | Self::REGISTER.0 | Self::ASSIGN_ROLES.0 | Self::EDIT_ROLES.0);
    /// Stable names, for the server files and the console.
    pub const NAMES: [(Self, &'static str); 6] = [
        (Self::KICK, "kick"),
        (Self::BAN, "ban"),
        (Self::CHANNELS, "channels"),
        (Self::REGISTER, "register"),
        (Self::ASSIGN_ROLES, "assign_roles"),
        (Self::EDIT_ROLES, "edit_roles"),
    ];

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Unknown bits are dropped.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// Has every right of `other`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Has at least one right of `other`.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES.iter().filter(|(p, _)| self.contains(*p)).map(|(_, n)| *n).collect()
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::NAMES.iter().find(|(_, n)| *n == name).map(|(p, _)| *p)
    }
}

impl std::ops::BitOr for Perms {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A role as the server defines it. `color` and `icon` are names from the
/// client's palette and icon set; a name the client does not know falls back
/// to the default look, so the server does not need to know the lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleInfo {
    pub id: RoleId,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub perms: Perms,
}

/// The server's roles, most senior first: the order is the hierarchy.
///
/// The rule for every action on another user (role, kick, ban, nickname
/// registration): the actor must be strictly above the target, see
/// [`Roles::outranks`]. Roles are given and edited only below one's own, and
/// a role cannot get a right its editor does not have.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Roles {
    pub list: Vec<RoleInfo>,
}

impl Roles {
    /// A new server: owner, admin (all rights) and moderator (kick), with
    /// ids 1, 2, 3.
    pub fn defaults() -> Self {
        let role = |id, name: &str, color: &str, icon: &str, perms| RoleInfo {
            id,
            name: name.into(),
            color: color.into(),
            icon: icon.into(),
            perms,
        };
        Self {
            list: vec![
                role(OWNER_ROLE, "Owner", "gold", OWNER_ICON, Perms::ALL),
                role(2, "Admin", "gold", "star-fill", Perms::ALL),
                role(3, "Moderator", "silver", "star", Perms::KICK),
            ],
        }
    }

    pub fn get(&self, id: RoleId) -> Option<&RoleInfo> {
        self.list.iter().find(|r| r.id == id)
    }

    pub fn index(&self, id: RoleId) -> Option<usize> {
        self.list.iter().position(|r| r.id == id)
    }

    /// Seniority: higher is more senior; no role (or an unknown one) is 0.
    pub fn rank(&self, role: Option<RoleId>) -> usize {
        role.and_then(|id| self.index(id)).map_or(0, |i| self.list.len() - i)
    }

    /// Rights of a role; the owner has all of them whatever the list says.
    pub fn perms(&self, role: Option<RoleId>) -> Perms {
        match role {
            Some(OWNER_ROLE) => Perms::ALL,
            Some(id) => self.get(id).map_or(Perms::NONE, |r| r.perms),
            None => Perms::NONE,
        }
    }

    /// Whether `actor` may act on a user with role `target`: kick, ban,
    /// change the role or the registration. Only on those strictly below; so
    /// never on oneself, an equal or someone higher.
    pub fn outranks(&self, actor: Option<RoleId>, target: Option<RoleId>) -> bool {
        self.rank(actor) > self.rank(target)
    }

    /// Whether `actor` may give `role` (`None`: take the role away): with
    /// the right to, only existing roles below their own, never the owner's.
    pub fn can_assign(&self, actor: Option<RoleId>, role: Option<RoleId>) -> bool {
        let below = match role {
            None => true,
            Some(OWNER_ROLE) => false,
            Some(id) => self.get(id).is_some() && self.rank(Some(id)) < self.rank(actor),
        };
        self.perms(actor).contains(Perms::ASSIGN_ROLES) && below
    }

    /// Whether `actor` may edit, move or delete `role`: with the right to,
    /// only roles below their own (so never the owner's).
    pub fn can_edit(&self, actor: Option<RoleId>, role: RoleId) -> bool {
        self.perms(actor).contains(Perms::EDIT_ROLES)
            && role != OWNER_ROLE
            && self.get(role).is_some()
            && self.rank(Some(role)) < self.rank(actor)
    }
}

/// Role name check: 1..=32 characters, no spaces at the ends, no line breaks
/// or other control characters, no `@` or `#` at the start (they read as
/// mentions and channels).
pub fn validate_role_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=MAX_ROLE_NAME_CHARS).contains(&count)
        && name.trim() == name
        && !name.chars().any(|c| c.is_control())
        && !name.starts_with(['@', '#'])
}

/// A color or icon name: short, lowercase letters, digits and dashes.
pub fn validate_style_name(name: &str) -> bool {
    (1..=24).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub id: UserId,
    pub nickname: String,
    pub channel_id: ChannelId,
    pub muted: bool,
    pub deafened: bool,
    pub role: Option<RoleId>,
    /// The nickname is registered to this user's key: nobody else can use it.
    pub registered: bool,
    /// The client presented a key (identity). Roles and registration need one.
    pub has_key: bool,
    /// Media streams the user is currently sending besides voice,
    /// e.g. screen share.
    pub streams: Vec<MediaKind>,
    /// Whose screen share this user is watching. Only a share in the same
    /// channel, cleared when it stops; the viewers of a share are the users
    /// watching it.
    pub watching: Option<UserId>,
}

/// A row of the admin panel: a registered user (online or not) or an
/// unregistered user who is online now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelUser {
    /// Public key fingerprint "AB:CD:...". `None`: online without a key.
    pub key: Option<String>,
    pub nickname: String,
    pub role: Option<RoleId>,
    pub registered: bool,
    /// Session id if the user is online now.
    pub online: Option<UserId>,
    /// Client version of an online user; empty if offline or not sent.
    pub client_version: String,
    /// Unix time of registration, 0 if not registered.
    pub created: i64,
}

/// A ban, for the admin panel. Sent back as is to lift it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BanInfo {
    pub nickname: Option<String>,
    pub ip: Option<String>,
    pub key: Option<String>,
    pub reason: String,
    pub created: i64,
    /// Unix time the ban ends; `None`: permanent.
    pub expires: Option<i64>,
    /// Nickname of the moderator who issued it; `None`: the server console.
    pub by: Option<String>,
}

/// Ban durations the client offers, in seconds; the server accepts any
/// duration up to `MAX_BAN_SECS`.
pub const BAN_DURATIONS: [u64; 4] = [3600, 86_400, 7 * 86_400, 30 * 86_400];
/// The longest temporary ban; longer is permanent.
pub const MAX_BAN_SECS: u64 = 366 * 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: u64,
    pub channel_id: ChannelId,
    pub author_id: UserId,
    pub author_name: String,
    pub text: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateMessage {
    pub from_id: UserId,
    pub from_name: String,
    pub to_id: UserId,
    pub to_name: String,
    pub text: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    /// First message after the stream is opened.
    Hello {
        /// Client version (`VERSION`), for logs and display.
        client_version: String,
        /// Optional features the client supports (`FEATURES`).
        features: Vec<String>,
        nickname: String,
        password: String,
        /// Media the client can receive. Without `Screen` the server does not
        /// forward screen shares to this client.
        capabilities: Vec<MediaKind>,
    },
    JoinChannel {
        channel_id: ChannelId,
    },
    ChatSend {
        channel_id: ChannelId,
        text: String,
    },
    PrivateSend {
        to: UserId,
        text: String,
    },
    SetVoiceState {
        muted: bool,
        deafened: bool,
    },
    /// The one-time setup code from the server log. Makes the sender's key
    /// the owner; works only while the server has no owner.
    AdminSetup {
        code: String,
    },
    /// Give a role (`None` takes it away). Registers the user's nickname to
    /// their key if it is not registered yet.
    SetRole {
        user_id: UserId,
        role: Option<RoleId>,
    },
    /// Register the user's nickname to their key, or remove the
    /// registration (and the role with it).
    SetRegistered {
        user_id: UserId,
        registered: bool,
    },
    /// Admin panel: registered users and everyone online (answer: `ServerUsers`).
    ListUsers,
    /// Admin panel: role of a registered user by key, online or not.
    /// Answer: an updated `ServerUsers`.
    SetRoleByKey {
        key: String,
        role: Option<RoleId>,
    },
    /// Admin panel: remove a registration by key, online or not.
    UnregisterKey {
        key: String,
    },
    /// Admin panel: list bans (answer: `Bans`).
    ListBans,
    /// Admin panel: lift a ban, given exactly as `Bans` listed it.
    /// Answer: an updated `Bans`.
    Unban {
        ban: BanInfo,
    },
    Kick {
        user_id: UserId,
        reason: String,
    },
    Ban {
        user_id: UserId,
        reason: String,
        /// `None`: permanent; otherwise up to `MAX_BAN_SECS`.
        duration_secs: Option<u64>,
    },
    /// Start sending a media stream, e.g. screen share.
    /// Voice is always on and is not controlled by this command.
    StartStream {
        kind: MediaKind,
    },
    StopStream {
        kind: MediaKind,
    },
    /// Watch the screen share of a user in your channel. `None`
    /// stops watching. Only one share can be watched at a time.
    WatchStream {
        user_id: Option<UserId>,
    },
    /// Ask the sharer for a keyframe: the viewer lost part of the
    /// video and cannot continue decoding.
    RequestKeyframe {
        user_id: UserId,
    },
    /// Admin: create a channel at the end of the list.
    CreateChannel {
        name: String,
        description: String,
        max_users: Option<u32>,
    },
    /// Admin: rename a channel, change its description or limit.
    /// If the new limit is below the current member count nobody is kicked;
    /// new users just cannot join until a slot frees up.
    EditChannel {
        channel_id: ChannelId,
        name: String,
        description: String,
        max_users: Option<u32>,
    },
    /// Admin: delete a channel. Its members are moved to the first
    /// channel with free space. The last channel cannot be deleted.
    DeleteChannel {
        channel_id: ChannelId,
    },
    /// Create a role at the bottom of the list (`role.id` is ignored).
    CreateRole {
        role: RoleInfo,
    },
    /// Change a role's name, color, icon and rights.
    EditRole {
        role: RoleInfo,
    },
    /// Delete a role; its users are left without a role.
    DeleteRole {
        role_id: RoleId,
    },
    /// Move a role to `index` in the list (0 is the top, which is the owner's).
    MoveRole {
        role_id: RoleId,
        index: u32,
    },
    /// The owner's server settings (answer: `ServerSettings`). Owner only.
    GetServerSettings,
    /// Set the server password; empty removes it (answer: `ServerSettings`).
    /// Owner only. See `validate_password`.
    SetPassword {
        password: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// A reason from a newer server that this version does not know.
    Other,
    WrongPassword,
    InvalidNickname,
    NicknameTaken,
    /// The nickname is registered to another key.
    NicknameReserved,
    Banned {
        reason: String,
        /// Seconds until the ban ends, `None`: permanent. Time left rather
        /// than an end time, so a client with a wrong clock still shows it
        /// right.
        remaining_secs: Option<u64>,
    },
    ServerFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// A code from a newer peer that this version does not know.
    Unknown,
    NotAdmin,
    NoSuchUser,
    NoSuchChannel,
    ChannelFull,
    MessageTooLong,
    CannotTargetSelf,
    Unsupported,
    /// The user is not sharing their screen in your channel.
    NotStreaming,
    /// Channel name is empty, too long or contains control characters,
    /// or the description is too long.
    InvalidChannelName,
    ChannelNameTaken,
    LastChannel,
    /// The action needs a key (identity), and the target client has none.
    IdentityRequired,
    /// Too many chat messages: this one was dropped.
    RateLimited,
    NoSuchRole,
    /// Role name empty, too long or with control characters, or a bad color
    /// or icon name.
    InvalidRoleName,
    RoleNameTaken,
    TooManyRoles,
    /// A new server password longer than `MAX_PASSWORD_CHARS` or with
    /// control characters or spaces at the ends.
    InvalidPassword,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    Welcome {
        your_id: UserId,
        server_name: String,
        motd: String,
        channels: Vec<ChannelInfo>,
        users: Vec<UserInfo>,
        /// The server has no owner yet and waits for the setup code.
        admin_setup: bool,
        /// Server version (`VERSION`), shown in the client.
        server_version: String,
        /// Optional features the server supports (`FEATURES`).
        features: Vec<String>,
        roles: Roles,
    },
    Rejected {
        reason: RejectReason,
    },
    UserJoined(UserInfo),
    UserLeft {
        user_id: UserId,
    },
    /// User changed channel, mute state or permissions.
    UserUpdated(UserInfo),
    /// Recent channel messages. Sent on joining a channel.
    ChannelHistory {
        channel_id: ChannelId,
        messages: Vec<ChatMessage>,
    },
    Chat(ChatMessage),
    Private(PrivateMessage),
    AdminSetupResult {
        ok: bool,
    },
    Kicked {
        reason: String,
        banned: bool,
        /// With `banned`: the ban's duration in seconds, `None`: permanent.
        ban_secs: Option<u64>,
    },
    /// Announcement from the admin via the server console.
    Notice {
        text: String,
    },
    Error {
        code: ErrorCode,
    },
    /// To the sharer: send a keyframe. Sent when a new viewer joins
    /// or a viewer lost packets.
    KeyframeRequest,
    ChannelAdded(ChannelInfo),
    /// Channel renamed or its description/limit changed.
    ChannelUpdated(ChannelInfo),
    /// Channel deleted. Member moves arrive earlier via UserUpdated.
    ChannelRemoved {
        channel_id: ChannelId,
    },
    /// Admin panel: registered users and everyone online. Also sent to an
    /// admin after role and registration changes.
    ServerUsers {
        users: Vec<PanelUser>,
    },
    /// Admin panel: all bans.
    Bans {
        bans: Vec<BanInfo>,
    },
    /// Server settings for the owner. The password itself is never sent.
    ServerSettings {
        password_set: bool,
    },
    /// The role list changed; sent to everyone.
    Roles(Roles),
}

/// Nickname check: 1 to 32 characters, no control characters,
/// no leading or trailing whitespace.
pub fn validate_nickname(nick: &str) -> bool {
    let count = nick.chars().count();
    (1..=MAX_NICKNAME_CHARS).contains(&count) && nick.trim() == nick && !nick.chars().any(|c| c.is_control())
}

/// Channel name check: same rules as for nicknames.
pub fn validate_channel_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=MAX_CHANNEL_NAME_CHARS).contains(&count) && name.trim() == name && !name.chars().any(|c| c.is_control())
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_features_only_those_needing_the_server() {
        use super::{Feature, missing_on_server};
        let ours = [
            Feature { name: "chat-history", needs_server: true },
            Feature { name: "new-theme", needs_server: false },
            Feature { name: "voice-v2", needs_server: true },
        ];
        let server = vec!["voice-v2".to_string(), "server-only".to_string()];
        let missing: Vec<&str> = missing_on_server(&ours, &server).iter().map(|f| f.name).collect();
        assert_eq!(missing, ["chat-history"], "client-only features and supported ones are not missing");
        assert!(missing_on_server(&ours[1..2], &[]).is_empty());
    }

    #[test]
    fn alpn_names() {
        assert_eq!(super::alpn_protocols()[0], b"chumly/1".to_vec());
        assert_eq!(super::major_from_alpn(b"chumly/12"), Some(12));
        assert_eq!(super::major_from_alpn(b"other/1"), None);
        assert_eq!(super::major_from_alpn(b"chumly/x"), None);
    }

    #[test]
    fn role_hierarchy() {
        let roles = Roles::defaults();
        let (o, a, m, n) = (Some(OWNER_ROLE), Some(2), Some(3), None);
        // Only strictly below: never oneself, an equal or someone higher.
        assert!(roles.outranks(o, a) && roles.outranks(o, m) && roles.outranks(o, n));
        assert!(roles.outranks(a, m) && roles.outranks(a, n) && !roles.outranks(a, a) && !roles.outranks(a, o));
        assert!(roles.outranks(m, n) && !roles.outranks(m, m) && !roles.outranks(m, a));
        assert!(!roles.outranks(n, n) && !roles.outranks(o, o));
        assert!(!roles.outranks(n, Some(99)) && roles.outranks(m, Some(99)), "an unknown role is no role");
        // Roles below one's own; the owner role never; moderators give none.
        assert!(roles.can_assign(o, a) && roles.can_assign(o, m) && roles.can_assign(o, n) && !roles.can_assign(o, o));
        assert!(roles.can_assign(a, m) && roles.can_assign(a, n) && !roles.can_assign(a, a) && !roles.can_assign(a, o));
        assert!(!roles.can_assign(m, n) && !roles.can_assign(m, m) && !roles.can_assign(n, n));
        assert!(!roles.can_assign(o, Some(99)), "an unknown role cannot be given");
        // Editing: below one's own, never the owner role.
        assert!(roles.can_edit(o, 2) && roles.can_edit(o, 3) && !roles.can_edit(o, OWNER_ROLE));
        assert!(roles.can_edit(a, 3) && !roles.can_edit(a, 2) && !roles.can_edit(m, 3));
    }

    #[test]
    fn rights_follow_the_list() {
        let mut roles = Roles::defaults();
        // The owner has all rights even if the list says otherwise.
        roles.list[0].perms = Perms::NONE;
        assert_eq!(roles.perms(Some(OWNER_ROLE)), Perms::ALL);
        // Moving the moderator above the admin makes it senior.
        roles.list.swap(1, 2);
        assert!(roles.outranks(Some(3), Some(2)));
        // Without the right, the position alone gives nothing.
        assert!(!roles.can_assign(Some(3), None));
        assert_eq!(Perms::from_bits(u32::MAX), Perms::ALL, "unknown bits are dropped");
        assert_eq!(Perms::from_name("ban"), Some(Perms::BAN));
        assert_eq!((Perms::KICK | Perms::BAN).names(), ["kick", "ban"]);
    }

    #[test]
    fn role_name_validation() {
        assert!(validate_role_name("Рейд-лидер"));
        assert!(validate_role_name("Event organizers & helpers"));
        assert!(validate_role_name("VIP #1"));
        assert!(!validate_role_name(""));
        assert!(!validate_role_name(" VIP"));
        assert!(!validate_role_name("VIP\n"));
        assert!(!validate_role_name("@everyone"));
        assert!(!validate_role_name("#general"));
        assert!(!validate_role_name(&"x".repeat(33)));
    }

    use super::*;

    #[test]
    fn nickname_validation() {
        assert!(validate_nickname("Алекс"));
        assert!(validate_nickname("a b"));
        assert!(!validate_nickname(""));
        assert!(!validate_nickname(" a"));
        assert!(!validate_nickname("a\n"));
        assert!(!validate_nickname(&"x".repeat(33)));
    }

    #[test]
    fn channel_name_validation() {
        assert!(validate_channel_name("Игры"));
        assert!(!validate_channel_name(""));
        assert!(!validate_channel_name("a "));
        assert!(!validate_channel_name("a	b"));
        assert!(!validate_channel_name(&"x".repeat(MAX_CHANNEL_NAME_CHARS + 1)));
    }
}
