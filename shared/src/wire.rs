//! Wire format of control messages: protobuf (prost), field numbers fixed.
//!
//! The application works with the Rust types in `protocol.rs`; this module
//! converts them to and from protobuf messages. Rules that keep different
//! versions of client and server working together:
//! - a new field gets a new tag; tags are never reused or renumbered,
//!   a removed field leaves its tag reserved (a comment here);
//! - unknown fields are skipped by protobuf itself;
//! - an unknown message type decodes to `Ok(None)` and the reader skips it;
//! - enums travel as plain numbers listed below. An unknown number in data
//!   that is only shown (a stream kind) is dropped; in a request (a stream
//!   to start) it makes the whole request unknown, so it is skipped rather
//!   than misread. Roles are data (ids into the server's role list), not an
//!   enum: an id the client does not find is shown as no role.

use prost::Message;

use crate::media::MediaKind;
use crate::protocol::{
    BanInfo, ChannelInfo, ChatMessage, ClientMsg, ErrorCode, PanelUser, Perms, PrivateMessage, RejectReason, RoleId,
    RoleInfo, Roles, ServerMsg, ServerStatus, UserInfo,
};

/// A message type that travels in frames (see `framing.rs`).
pub trait Wire: Sized {
    fn encode_wire(&self) -> Vec<u8>;
    /// `Ok(None)`: a message type this version does not know; skip it.
    fn decode_wire(bytes: &[u8]) -> anyhow::Result<Option<Self>>;
}

// ---------------------------------------------------------------------------
// Enums as numbers. 0 is "none / unknown" in every list.
// ---------------------------------------------------------------------------

/// A role id on the wire: 0 is no role.
fn role_to(role: Option<RoleId>) -> u32 {
    role.unwrap_or(0)
}

fn role_from(v: u32) -> Option<RoleId> {
    (v != 0).then_some(v)
}

fn kind_to(k: MediaKind) -> u32 {
    k as u32
}

fn kind_from(v: u32) -> Option<MediaKind> {
    u8::try_from(v).ok().and_then(MediaKind::from_u8)
}

fn kinds_to(kinds: &[MediaKind]) -> Vec<u32> {
    kinds.iter().map(|k| kind_to(*k)).collect()
}

/// Unknown kinds are dropped: this version cannot use them anyway.
fn kinds_from(v: &[u32]) -> Vec<MediaKind> {
    v.iter().filter_map(|k| kind_from(*k)).collect()
}

fn error_to(code: ErrorCode) -> i32 {
    match code {
        ErrorCode::Unknown => 0,
        ErrorCode::NotAdmin => 1,
        ErrorCode::NoSuchUser => 2,
        ErrorCode::NoSuchChannel => 3,
        ErrorCode::ChannelFull => 4,
        ErrorCode::MessageTooLong => 5,
        ErrorCode::CannotTargetSelf => 6,
        ErrorCode::Unsupported => 7,
        ErrorCode::NotStreaming => 8,
        ErrorCode::InvalidChannelName => 9,
        ErrorCode::ChannelNameTaken => 10,
        ErrorCode::LastChannel => 11,
        ErrorCode::IdentityRequired => 12,
        ErrorCode::RateLimited => 13,
        ErrorCode::NoSuchRole => 14,
        ErrorCode::InvalidRoleName => 15,
        ErrorCode::RoleNameTaken => 16,
        ErrorCode::TooManyRoles => 17,
        ErrorCode::InvalidPassword => 18,
    }
}

fn error_from(v: i32) -> ErrorCode {
    match v {
        1 => ErrorCode::NotAdmin,
        2 => ErrorCode::NoSuchUser,
        3 => ErrorCode::NoSuchChannel,
        4 => ErrorCode::ChannelFull,
        5 => ErrorCode::MessageTooLong,
        6 => ErrorCode::CannotTargetSelf,
        7 => ErrorCode::Unsupported,
        8 => ErrorCode::NotStreaming,
        9 => ErrorCode::InvalidChannelName,
        10 => ErrorCode::ChannelNameTaken,
        11 => ErrorCode::LastChannel,
        12 => ErrorCode::IdentityRequired,
        13 => ErrorCode::RateLimited,
        14 => ErrorCode::NoSuchRole,
        15 => ErrorCode::InvalidRoleName,
        16 => ErrorCode::RoleNameTaken,
        17 => ErrorCode::TooManyRoles,
        18 => ErrorCode::InvalidPassword,
        _ => ErrorCode::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Shared structures.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Message)]
pub struct WChannel {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub description: String,
    #[prost(uint32, optional, tag = "4")]
    pub max_users: Option<u32>,
}

impl From<&ChannelInfo> for WChannel {
    fn from(c: &ChannelInfo) -> Self {
        Self { id: c.id, name: c.name.clone(), description: c.description.clone(), max_users: c.max_users }
    }
}

impl From<WChannel> for ChannelInfo {
    fn from(c: WChannel) -> Self {
        Self { id: c.id, name: c.name, description: c.description, max_users: c.max_users }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WUser {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(string, tag = "2")]
    pub nickname: String,
    #[prost(uint32, tag = "3")]
    pub channel_id: u32,
    #[prost(bool, tag = "4")]
    pub muted: bool,
    #[prost(bool, tag = "5")]
    pub deafened: bool,
    #[prost(uint32, tag = "6")]
    pub role: u32,
    #[prost(bool, tag = "7")]
    pub registered: bool,
    #[prost(bool, tag = "8")]
    pub has_key: bool,
    #[prost(uint32, repeated, tag = "9")]
    pub streams: Vec<u32>,
    #[prost(uint32, optional, tag = "10")]
    pub watching: Option<u32>,
}

impl From<&UserInfo> for WUser {
    fn from(u: &UserInfo) -> Self {
        Self {
            id: u.id,
            nickname: u.nickname.clone(),
            channel_id: u.channel_id,
            muted: u.muted,
            deafened: u.deafened,
            role: role_to(u.role),
            registered: u.registered,
            has_key: u.has_key,
            streams: kinds_to(&u.streams),
            watching: u.watching,
        }
    }
}

impl From<WUser> for UserInfo {
    fn from(u: WUser) -> Self {
        Self {
            id: u.id,
            nickname: u.nickname,
            channel_id: u.channel_id,
            muted: u.muted,
            deafened: u.deafened,
            role: role_from(u.role),
            registered: u.registered,
            has_key: u.has_key,
            streams: kinds_from(&u.streams),
            watching: u.watching,
        }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WPanelUser {
    #[prost(string, optional, tag = "1")]
    pub key: Option<String>,
    #[prost(string, tag = "2")]
    pub nickname: String,
    #[prost(uint32, tag = "3")]
    pub role: u32,
    #[prost(bool, tag = "4")]
    pub registered: bool,
    #[prost(uint32, optional, tag = "5")]
    pub online: Option<u32>,
    #[prost(int64, tag = "6")]
    pub created: i64,
    #[prost(string, tag = "7")]
    pub client_version: String,
}

impl From<&PanelUser> for WPanelUser {
    fn from(u: &PanelUser) -> Self {
        Self {
            key: u.key.clone(),
            nickname: u.nickname.clone(),
            role: role_to(u.role),
            registered: u.registered,
            online: u.online,
            created: u.created,
            client_version: u.client_version.clone(),
        }
    }
}

impl From<WPanelUser> for PanelUser {
    fn from(u: WPanelUser) -> Self {
        Self {
            key: u.key,
            nickname: u.nickname,
            role: role_from(u.role),
            registered: u.registered,
            online: u.online,
            created: u.created,
            client_version: u.client_version,
        }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WRole {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub color: String,
    #[prost(string, tag = "4")]
    pub icon: String,
    #[prost(uint32, tag = "5")]
    pub perms: u32,
}

impl From<&RoleInfo> for WRole {
    fn from(r: &RoleInfo) -> Self {
        Self { id: r.id, name: r.name.clone(), color: r.color.clone(), icon: r.icon.clone(), perms: r.perms.bits() }
    }
}

impl From<WRole> for RoleInfo {
    fn from(r: WRole) -> Self {
        Self { id: r.id, name: r.name, color: r.color, icon: r.icon, perms: Perms::from_bits(r.perms) }
    }
}

/// The role list, most senior first.
#[derive(Clone, PartialEq, Message)]
pub struct WRoles {
    #[prost(message, repeated, tag = "1")]
    pub roles: Vec<WRole>,
}

impl From<&Roles> for WRoles {
    fn from(r: &Roles) -> Self {
        Self { roles: r.list.iter().map(Into::into).collect() }
    }
}

impl From<WRoles> for Roles {
    fn from(r: WRoles) -> Self {
        Self { list: r.roles.into_iter().map(Into::into).collect() }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WRoleRef {
    #[prost(uint32, tag = "1")]
    pub role_id: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WMoveRole {
    #[prost(uint32, tag = "1")]
    pub role_id: u32,
    #[prost(uint32, tag = "2")]
    pub index: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WBan {
    #[prost(string, optional, tag = "1")]
    pub nickname: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub ip: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub key: Option<String>,
    #[prost(string, tag = "4")]
    pub reason: String,
    #[prost(int64, tag = "5")]
    pub created: i64,
    #[prost(int64, optional, tag = "6")]
    pub expires: Option<i64>,
    #[prost(string, optional, tag = "7")]
    pub by: Option<String>,
}

impl From<&BanInfo> for WBan {
    fn from(b: &BanInfo) -> Self {
        Self {
            nickname: b.nickname.clone(),
            ip: b.ip.clone(),
            key: b.key.clone(),
            reason: b.reason.clone(),
            created: b.created,
            expires: b.expires,
            by: b.by.clone(),
        }
    }
}

impl From<WBan> for BanInfo {
    fn from(b: WBan) -> Self {
        Self {
            nickname: b.nickname,
            ip: b.ip,
            key: b.key,
            reason: b.reason,
            created: b.created,
            expires: b.expires,
            by: b.by,
        }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WChat {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(uint32, tag = "2")]
    pub channel_id: u32,
    #[prost(uint32, tag = "3")]
    pub author_id: u32,
    #[prost(string, tag = "4")]
    pub author_name: String,
    #[prost(string, tag = "5")]
    pub text: String,
    #[prost(int64, tag = "6")]
    pub timestamp: i64,
}

impl From<&ChatMessage> for WChat {
    fn from(m: &ChatMessage) -> Self {
        Self {
            id: m.id,
            channel_id: m.channel_id,
            author_id: m.author_id,
            author_name: m.author_name.clone(),
            text: m.text.clone(),
            timestamp: m.timestamp,
        }
    }
}

impl From<WChat> for ChatMessage {
    fn from(m: WChat) -> Self {
        Self {
            id: m.id,
            channel_id: m.channel_id,
            author_id: m.author_id,
            author_name: m.author_name,
            text: m.text,
            timestamp: m.timestamp,
        }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WPrivate {
    #[prost(uint32, tag = "1")]
    pub from_id: u32,
    #[prost(string, tag = "2")]
    pub from_name: String,
    #[prost(uint32, tag = "3")]
    pub to_id: u32,
    #[prost(string, tag = "4")]
    pub to_name: String,
    #[prost(string, tag = "5")]
    pub text: String,
    #[prost(int64, tag = "6")]
    pub timestamp: i64,
}

impl From<&PrivateMessage> for WPrivate {
    fn from(m: &PrivateMessage) -> Self {
        Self {
            from_id: m.from_id,
            from_name: m.from_name.clone(),
            to_id: m.to_id,
            to_name: m.to_name.clone(),
            text: m.text.clone(),
            timestamp: m.timestamp,
        }
    }
}

impl From<WPrivate> for PrivateMessage {
    fn from(m: WPrivate) -> Self {
        Self {
            from_id: m.from_id,
            from_name: m.from_name,
            to_id: m.to_id,
            to_name: m.to_name,
            text: m.text,
            timestamp: m.timestamp,
        }
    }
}

/// `RejectReason` as a code plus the fields some reasons carry.
#[derive(Clone, PartialEq, Message)]
pub struct WReject {
    /// 1 wrong password, 2 invalid nickname, 3 nickname taken, 4 nickname
    /// reserved, 5 banned, 6 server full.
    #[prost(int32, tag = "1")]
    pub code: i32,
    #[prost(string, tag = "2")]
    pub reason: String,
    /// Banned: seconds until the ban ends; absent: permanent.
    #[prost(uint64, optional, tag = "3")]
    pub remaining_secs: Option<u64>,
}

impl From<&RejectReason> for WReject {
    fn from(r: &RejectReason) -> Self {
        let (code, reason, remaining_secs) = match r {
            RejectReason::Other => (0, String::new(), None),
            RejectReason::WrongPassword => (1, String::new(), None),
            RejectReason::InvalidNickname => (2, String::new(), None),
            RejectReason::NicknameTaken => (3, String::new(), None),
            RejectReason::NicknameReserved => (4, String::new(), None),
            RejectReason::Banned { reason, remaining_secs } => (5, reason.clone(), *remaining_secs),
            RejectReason::ServerFull => (6, String::new(), None),
        };
        Self { code, reason, remaining_secs }
    }
}

impl From<WReject> for RejectReason {
    fn from(r: WReject) -> Self {
        match r.code {
            1 => RejectReason::WrongPassword,
            2 => RejectReason::InvalidNickname,
            3 => RejectReason::NicknameTaken,
            4 => RejectReason::NicknameReserved,
            5 => RejectReason::Banned { reason: r.reason, remaining_secs: r.remaining_secs },
            6 => RejectReason::ServerFull,
            _ => RejectReason::Other,
        }
    }
}

// ---------------------------------------------------------------------------
// Client -> server.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Message)]
pub struct WClientMsg {
    #[prost(
        oneof = "WClient",
        tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28"
    )]
    pub msg: Option<WClient>,
}

#[derive(Clone, PartialEq, prost::Oneof)]
pub enum WClient {
    #[prost(message, tag = "1")]
    Hello(WHello),
    #[prost(message, tag = "2")]
    JoinChannel(WChannelRef),
    #[prost(message, tag = "3")]
    ChatSend(WChatSend),
    #[prost(message, tag = "4")]
    PrivateSend(WPrivateSend),
    #[prost(message, tag = "5")]
    SetVoiceState(WVoiceState),
    #[prost(message, tag = "6")]
    AdminSetup(WAdminSetup),
    #[prost(message, tag = "7")]
    SetRole(WSetRole),
    #[prost(message, tag = "8")]
    SetRegistered(WSetRegistered),
    #[prost(message, tag = "9")]
    ListUsers(WEmpty),
    #[prost(message, tag = "10")]
    SetRoleByKey(WSetRoleByKey),
    #[prost(message, tag = "11")]
    UnregisterKey(WKey),
    #[prost(message, tag = "12")]
    ListBans(WEmpty),
    #[prost(message, tag = "13")]
    Unban(WUnban),
    #[prost(message, tag = "14")]
    Kick(WModerate),
    #[prost(message, tag = "15")]
    Ban(WModerate),
    #[prost(message, tag = "16")]
    StartStream(WStream),
    #[prost(message, tag = "17")]
    StopStream(WStream),
    #[prost(message, tag = "18")]
    WatchStream(WWatch),
    #[prost(message, tag = "19")]
    RequestKeyframe(WUserRef),
    #[prost(message, tag = "20")]
    CreateChannel(WChannelEdit),
    #[prost(message, tag = "21")]
    EditChannel(WChannelEdit),
    #[prost(message, tag = "22")]
    DeleteChannel(WChannelRef),
    #[prost(message, tag = "23")]
    CreateRole(WRole),
    #[prost(message, tag = "24")]
    EditRole(WRole),
    #[prost(message, tag = "25")]
    DeleteRole(WRoleRef),
    #[prost(message, tag = "26")]
    MoveRole(WMoveRole),
    #[prost(message, tag = "27")]
    GetServerSettings(WEmpty),
    #[prost(message, tag = "28")]
    SetPassword(WPassword),
}

#[derive(Clone, PartialEq, Message)]
pub struct WPassword {
    #[prost(string, tag = "1")]
    pub password: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WServerSettings {
    #[prost(bool, tag = "1")]
    pub password_set: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct WEmpty {}

#[derive(Clone, PartialEq, Message)]
pub struct WHello {
    #[prost(string, tag = "1")]
    pub client_version: String,
    #[prost(string, repeated, tag = "2")]
    pub features: Vec<String>,
    #[prost(string, tag = "3")]
    pub nickname: String,
    #[prost(string, tag = "4")]
    pub password: String,
    #[prost(uint32, repeated, tag = "5")]
    pub capabilities: Vec<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WChannelRef {
    #[prost(uint32, tag = "1")]
    pub channel_id: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WUserRef {
    #[prost(uint32, tag = "1")]
    pub user_id: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WChatSend {
    #[prost(uint32, tag = "1")]
    pub channel_id: u32,
    #[prost(string, tag = "2")]
    pub text: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WPrivateSend {
    #[prost(uint32, tag = "1")]
    pub to: u32,
    #[prost(string, tag = "2")]
    pub text: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WVoiceState {
    #[prost(bool, tag = "1")]
    pub muted: bool,
    #[prost(bool, tag = "2")]
    pub deafened: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct WAdminSetup {
    #[prost(string, tag = "1")]
    pub code: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WSetRole {
    #[prost(uint32, tag = "1")]
    pub user_id: u32,
    #[prost(uint32, tag = "2")]
    pub role: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WSetRegistered {
    #[prost(uint32, tag = "1")]
    pub user_id: u32,
    #[prost(bool, tag = "2")]
    pub registered: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct WSetRoleByKey {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(uint32, tag = "2")]
    pub role: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WKey {
    #[prost(string, tag = "1")]
    pub key: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WUnban {
    #[prost(message, optional, tag = "1")]
    pub ban: Option<WBan>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WModerate {
    #[prost(uint32, tag = "1")]
    pub user_id: u32,
    #[prost(string, tag = "2")]
    pub reason: String,
    /// Ban only: the duration in seconds; absent: permanent.
    #[prost(uint64, optional, tag = "3")]
    pub duration_secs: Option<u64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WStream {
    #[prost(uint32, tag = "1")]
    pub kind: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WWatch {
    #[prost(uint32, optional, tag = "1")]
    pub user_id: Option<u32>,
}

/// Create (no `channel_id`) or edit a channel.
#[derive(Clone, PartialEq, Message)]
pub struct WChannelEdit {
    #[prost(uint32, tag = "1")]
    pub channel_id: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub description: String,
    #[prost(uint32, optional, tag = "4")]
    pub max_users: Option<u32>,
}

impl From<&ClientMsg> for WClient {
    fn from(m: &ClientMsg) -> Self {
        match m {
            ClientMsg::Hello { client_version, features, nickname, password, capabilities } => WClient::Hello(WHello {
                client_version: client_version.clone(),
                features: features.clone(),
                nickname: nickname.clone(),
                password: password.clone(),
                capabilities: kinds_to(capabilities),
            }),
            ClientMsg::JoinChannel { channel_id } => WClient::JoinChannel(WChannelRef { channel_id: *channel_id }),
            ClientMsg::ChatSend { channel_id, text } => {
                WClient::ChatSend(WChatSend { channel_id: *channel_id, text: text.clone() })
            }
            ClientMsg::PrivateSend { to, text } => WClient::PrivateSend(WPrivateSend { to: *to, text: text.clone() }),
            ClientMsg::SetVoiceState { muted, deafened } => {
                WClient::SetVoiceState(WVoiceState { muted: *muted, deafened: *deafened })
            }
            ClientMsg::AdminSetup { code } => WClient::AdminSetup(WAdminSetup { code: code.clone() }),
            ClientMsg::SetRole { user_id, role } => {
                WClient::SetRole(WSetRole { user_id: *user_id, role: role_to(*role) })
            }
            ClientMsg::SetRegistered { user_id, registered } => {
                WClient::SetRegistered(WSetRegistered { user_id: *user_id, registered: *registered })
            }
            ClientMsg::ListUsers => WClient::ListUsers(WEmpty {}),
            ClientMsg::SetRoleByKey { key, role } => {
                WClient::SetRoleByKey(WSetRoleByKey { key: key.clone(), role: role_to(*role) })
            }
            ClientMsg::UnregisterKey { key } => WClient::UnregisterKey(WKey { key: key.clone() }),
            ClientMsg::ListBans => WClient::ListBans(WEmpty {}),
            ClientMsg::Unban { ban } => WClient::Unban(WUnban { ban: Some(ban.into()) }),
            ClientMsg::Kick { user_id, reason } => {
                WClient::Kick(WModerate { user_id: *user_id, reason: reason.clone(), duration_secs: None })
            }
            ClientMsg::Ban { user_id, reason, duration_secs } => {
                WClient::Ban(WModerate { user_id: *user_id, reason: reason.clone(), duration_secs: *duration_secs })
            }
            ClientMsg::StartStream { kind } => WClient::StartStream(WStream { kind: kind_to(*kind) }),
            ClientMsg::StopStream { kind } => WClient::StopStream(WStream { kind: kind_to(*kind) }),
            ClientMsg::WatchStream { user_id } => WClient::WatchStream(WWatch { user_id: *user_id }),
            ClientMsg::RequestKeyframe { user_id } => WClient::RequestKeyframe(WUserRef { user_id: *user_id }),
            ClientMsg::CreateChannel { name, description, max_users } => WClient::CreateChannel(WChannelEdit {
                channel_id: 0,
                name: name.clone(),
                description: description.clone(),
                max_users: *max_users,
            }),
            ClientMsg::EditChannel { channel_id, name, description, max_users } => WClient::EditChannel(WChannelEdit {
                channel_id: *channel_id,
                name: name.clone(),
                description: description.clone(),
                max_users: *max_users,
            }),
            ClientMsg::DeleteChannel { channel_id } => WClient::DeleteChannel(WChannelRef { channel_id: *channel_id }),
            ClientMsg::CreateRole { role } => WClient::CreateRole(role.into()),
            ClientMsg::EditRole { role } => WClient::EditRole(role.into()),
            ClientMsg::DeleteRole { role_id } => WClient::DeleteRole(WRoleRef { role_id: *role_id }),
            ClientMsg::MoveRole { role_id, index } => WClient::MoveRole(WMoveRole { role_id: *role_id, index: *index }),
            ClientMsg::GetServerSettings => WClient::GetServerSettings(WEmpty {}),
            ClientMsg::SetPassword { password } => WClient::SetPassword(WPassword { password: password.clone() }),
        }
    }
}

/// `None`: the request carries a value this version does not know.
fn client_from(m: WClient) -> anyhow::Result<Option<ClientMsg>> {
    Ok(Some(match m {
        WClient::Hello(h) => ClientMsg::Hello {
            client_version: h.client_version,
            features: h.features,
            nickname: h.nickname,
            password: h.password,
            capabilities: kinds_from(&h.capabilities),
        },
        WClient::JoinChannel(c) => ClientMsg::JoinChannel { channel_id: c.channel_id },
        WClient::ChatSend(c) => ClientMsg::ChatSend { channel_id: c.channel_id, text: c.text },
        WClient::PrivateSend(p) => ClientMsg::PrivateSend { to: p.to, text: p.text },
        WClient::SetVoiceState(v) => ClientMsg::SetVoiceState { muted: v.muted, deafened: v.deafened },
        WClient::AdminSetup(a) => ClientMsg::AdminSetup { code: a.code },
        WClient::SetRole(r) => ClientMsg::SetRole { user_id: r.user_id, role: role_from(r.role) },
        WClient::SetRegistered(r) => ClientMsg::SetRegistered { user_id: r.user_id, registered: r.registered },
        WClient::ListUsers(_) => ClientMsg::ListUsers,
        WClient::SetRoleByKey(r) => ClientMsg::SetRoleByKey { key: r.key, role: role_from(r.role) },
        WClient::UnregisterKey(k) => ClientMsg::UnregisterKey { key: k.key },
        WClient::ListBans(_) => ClientMsg::ListBans,
        WClient::Unban(u) => {
            let ban = u.ban.ok_or_else(|| anyhow::anyhow!("Unban without a ban"))?;
            ClientMsg::Unban { ban: ban.into() }
        }
        WClient::Kick(k) => ClientMsg::Kick { user_id: k.user_id, reason: k.reason },
        WClient::Ban(b) => ClientMsg::Ban { user_id: b.user_id, reason: b.reason, duration_secs: b.duration_secs },
        WClient::StartStream(s) => {
            let Some(kind) = kind_from(s.kind) else { return Ok(None) };
            ClientMsg::StartStream { kind }
        }
        WClient::StopStream(s) => {
            let Some(kind) = kind_from(s.kind) else { return Ok(None) };
            ClientMsg::StopStream { kind }
        }
        WClient::WatchStream(w) => ClientMsg::WatchStream { user_id: w.user_id },
        WClient::RequestKeyframe(r) => ClientMsg::RequestKeyframe { user_id: r.user_id },
        WClient::CreateChannel(c) => {
            ClientMsg::CreateChannel { name: c.name, description: c.description, max_users: c.max_users }
        }
        WClient::EditChannel(c) => ClientMsg::EditChannel {
            channel_id: c.channel_id,
            name: c.name,
            description: c.description,
            max_users: c.max_users,
        },
        WClient::DeleteChannel(c) => ClientMsg::DeleteChannel { channel_id: c.channel_id },
        WClient::CreateRole(r) => ClientMsg::CreateRole { role: r.into() },
        WClient::EditRole(r) => ClientMsg::EditRole { role: r.into() },
        WClient::DeleteRole(r) => ClientMsg::DeleteRole { role_id: r.role_id },
        WClient::MoveRole(m) => ClientMsg::MoveRole { role_id: m.role_id, index: m.index },
        WClient::GetServerSettings(_) => ClientMsg::GetServerSettings,
        WClient::SetPassword(p) => ClientMsg::SetPassword { password: p.password },
    }))
}

impl Wire for ClientMsg {
    fn encode_wire(&self) -> Vec<u8> {
        WClientMsg { msg: Some(self.into()) }.encode_to_vec()
    }

    fn decode_wire(bytes: &[u8]) -> anyhow::Result<Option<Self>> {
        match WClientMsg::decode(bytes)?.msg {
            Some(m) => client_from(m),
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Server -> client.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Message)]
pub struct WServerMsg {
    #[prost(oneof = "WServer", tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20")]
    pub msg: Option<WServer>,
}

#[derive(Clone, PartialEq, prost::Oneof)]
pub enum WServer {
    #[prost(message, tag = "1")]
    Welcome(WWelcome),
    #[prost(message, tag = "2")]
    Rejected(WReject),
    #[prost(message, tag = "3")]
    UserJoined(WUser),
    #[prost(message, tag = "4")]
    UserLeft(WUserRef),
    #[prost(message, tag = "5")]
    UserUpdated(WUser),
    #[prost(message, tag = "6")]
    ChannelHistory(WHistory),
    #[prost(message, tag = "7")]
    Chat(WChat),
    #[prost(message, tag = "8")]
    Private(WPrivate),
    #[prost(message, tag = "9")]
    AdminSetupResult(WOk),
    #[prost(message, tag = "10")]
    Kicked(WKicked),
    #[prost(message, tag = "11")]
    Notice(WNotice),
    #[prost(message, tag = "12")]
    Error(WError),
    #[prost(message, tag = "13")]
    KeyframeRequest(WEmpty),
    #[prost(message, tag = "14")]
    ChannelAdded(WChannel),
    #[prost(message, tag = "15")]
    ChannelUpdated(WChannel),
    #[prost(message, tag = "16")]
    ChannelRemoved(WChannelRef),
    #[prost(message, tag = "17")]
    ServerUsers(WServerUsers),
    #[prost(message, tag = "18")]
    Bans(WBans),
    #[prost(message, tag = "19")]
    Roles(WRoles),
    #[prost(message, tag = "20")]
    ServerSettings(WServerSettings),
}

#[derive(Clone, PartialEq, Message)]
pub struct WWelcome {
    #[prost(uint32, tag = "1")]
    pub your_id: u32,
    #[prost(string, tag = "2")]
    pub server_name: String,
    #[prost(string, tag = "3")]
    pub motd: String,
    #[prost(message, repeated, tag = "4")]
    pub channels: Vec<WChannel>,
    #[prost(message, repeated, tag = "5")]
    pub users: Vec<WUser>,
    #[prost(bool, tag = "6")]
    pub admin_setup: bool,
    #[prost(string, tag = "7")]
    pub server_version: String,
    #[prost(string, repeated, tag = "8")]
    pub features: Vec<String>,
    #[prost(message, repeated, tag = "9")]
    pub roles: Vec<WRole>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WHistory {
    #[prost(uint32, tag = "1")]
    pub channel_id: u32,
    #[prost(message, repeated, tag = "2")]
    pub messages: Vec<WChat>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WOk {
    #[prost(bool, tag = "1")]
    pub ok: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct WKicked {
    #[prost(string, tag = "1")]
    pub reason: String,
    #[prost(bool, tag = "2")]
    pub banned: bool,
    /// With `banned`: the ban's duration in seconds; absent: permanent.
    #[prost(uint64, optional, tag = "3")]
    pub ban_secs: Option<u64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WNotice {
    #[prost(string, tag = "1")]
    pub text: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WError {
    #[prost(int32, tag = "1")]
    pub code: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct WServerUsers {
    #[prost(message, repeated, tag = "1")]
    pub users: Vec<WPanelUser>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WBans {
    #[prost(message, repeated, tag = "1")]
    pub bans: Vec<WBan>,
}

impl From<&ServerMsg> for WServer {
    fn from(m: &ServerMsg) -> Self {
        match m {
            ServerMsg::Welcome {
                your_id,
                server_name,
                motd,
                channels,
                users,
                admin_setup,
                server_version,
                features,
                roles,
            } => WServer::Welcome(WWelcome {
                your_id: *your_id,
                server_name: server_name.clone(),
                motd: motd.clone(),
                channels: channels.iter().map(Into::into).collect(),
                users: users.iter().map(Into::into).collect(),
                admin_setup: *admin_setup,
                server_version: server_version.clone(),
                features: features.clone(),
                roles: roles.list.iter().map(Into::into).collect(),
            }),
            ServerMsg::Rejected { reason } => WServer::Rejected(reason.into()),
            ServerMsg::UserJoined(u) => WServer::UserJoined(u.into()),
            ServerMsg::UserLeft { user_id } => WServer::UserLeft(WUserRef { user_id: *user_id }),
            ServerMsg::UserUpdated(u) => WServer::UserUpdated(u.into()),
            ServerMsg::ChannelHistory { channel_id, messages } => WServer::ChannelHistory(WHistory {
                channel_id: *channel_id,
                messages: messages.iter().map(Into::into).collect(),
            }),
            ServerMsg::Chat(c) => WServer::Chat(c.into()),
            ServerMsg::Private(p) => WServer::Private(p.into()),
            ServerMsg::AdminSetupResult { ok } => WServer::AdminSetupResult(WOk { ok: *ok }),
            ServerMsg::Kicked { reason, banned, ban_secs } => {
                WServer::Kicked(WKicked { reason: reason.clone(), banned: *banned, ban_secs: *ban_secs })
            }
            ServerMsg::Notice { text } => WServer::Notice(WNotice { text: text.clone() }),
            ServerMsg::Error { code } => WServer::Error(WError { code: error_to(*code) }),
            ServerMsg::KeyframeRequest => WServer::KeyframeRequest(WEmpty {}),
            ServerMsg::ChannelAdded(c) => WServer::ChannelAdded(c.into()),
            ServerMsg::ChannelUpdated(c) => WServer::ChannelUpdated(c.into()),
            ServerMsg::ChannelRemoved { channel_id } => {
                WServer::ChannelRemoved(WChannelRef { channel_id: *channel_id })
            }
            ServerMsg::ServerUsers { users } => {
                WServer::ServerUsers(WServerUsers { users: users.iter().map(Into::into).collect() })
            }
            ServerMsg::Bans { bans } => WServer::Bans(WBans { bans: bans.iter().map(Into::into).collect() }),
            ServerMsg::ServerSettings { password_set } => {
                WServer::ServerSettings(WServerSettings { password_set: *password_set })
            }
            ServerMsg::Roles(r) => WServer::Roles(r.into()),
        }
    }
}

fn server_from(m: WServer) -> ServerMsg {
    match m {
        WServer::Welcome(w) => ServerMsg::Welcome {
            your_id: w.your_id,
            server_name: w.server_name,
            motd: w.motd,
            channels: w.channels.into_iter().map(Into::into).collect(),
            users: w.users.into_iter().map(Into::into).collect(),
            admin_setup: w.admin_setup,
            server_version: w.server_version,
            features: w.features,
            roles: Roles { list: w.roles.into_iter().map(Into::into).collect() },
        },
        WServer::Rejected(r) => ServerMsg::Rejected { reason: r.into() },
        WServer::UserJoined(u) => ServerMsg::UserJoined(u.into()),
        WServer::UserLeft(u) => ServerMsg::UserLeft { user_id: u.user_id },
        WServer::UserUpdated(u) => ServerMsg::UserUpdated(u.into()),
        WServer::ChannelHistory(h) => ServerMsg::ChannelHistory {
            channel_id: h.channel_id,
            messages: h.messages.into_iter().map(Into::into).collect(),
        },
        WServer::Chat(c) => ServerMsg::Chat(c.into()),
        WServer::Private(p) => ServerMsg::Private(p.into()),
        WServer::AdminSetupResult(o) => ServerMsg::AdminSetupResult { ok: o.ok },
        WServer::Kicked(k) => ServerMsg::Kicked { reason: k.reason, banned: k.banned, ban_secs: k.ban_secs },
        WServer::Notice(n) => ServerMsg::Notice { text: n.text },
        WServer::Error(e) => ServerMsg::Error { code: error_from(e.code) },
        WServer::KeyframeRequest(_) => ServerMsg::KeyframeRequest,
        WServer::ChannelAdded(c) => ServerMsg::ChannelAdded(c.into()),
        WServer::ChannelUpdated(c) => ServerMsg::ChannelUpdated(c.into()),
        WServer::ChannelRemoved(c) => ServerMsg::ChannelRemoved { channel_id: c.channel_id },
        WServer::ServerUsers(s) => ServerMsg::ServerUsers { users: s.users.into_iter().map(Into::into).collect() },
        WServer::Bans(b) => ServerMsg::Bans { bans: b.bans.into_iter().map(Into::into).collect() },
        WServer::ServerSettings(s) => ServerMsg::ServerSettings { password_set: s.password_set },
        WServer::Roles(r) => ServerMsg::Roles(r.into()),
    }
}

impl Wire for ServerMsg {
    fn encode_wire(&self) -> Vec<u8> {
        WServerMsg { msg: Some(self.into()) }.encode_to_vec()
    }

    fn decode_wire(bytes: &[u8]) -> anyhow::Result<Option<Self>> {
        Ok(WServerMsg::decode(bytes)?.msg.map(server_from))
    }
}

// ---------------------------------------------------------------------------
// Status query (see `protocol::STATUS_ALPN`): one message, no framing.
// ---------------------------------------------------------------------------

/// Largest status answer a client accepts.
pub const MAX_STATUS_BYTES: usize = 4096;

#[derive(Clone, PartialEq, Message)]
pub struct WStatus {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub version: String,
    #[prost(uint32, tag = "3")]
    pub users_online: u32,
    #[prost(uint32, tag = "4")]
    pub max_users: u32,
    #[prost(string, repeated, tag = "5")]
    pub features: Vec<String>,
}

pub fn encode_status(s: &ServerStatus) -> Vec<u8> {
    WStatus {
        name: s.name.clone(),
        version: s.version.clone(),
        users_online: s.users_online,
        max_users: s.max_users,
        features: s.features.clone(),
    }
    .encode_to_vec()
}

pub fn decode_status(bytes: &[u8]) -> anyhow::Result<ServerStatus> {
    let w = WStatus::decode(bytes)?;
    Ok(ServerStatus {
        name: w.name,
        version: w.version,
        users_online: w.users_online,
        max_users: w.max_users,
        features: w.features,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrips() {
        let s = ServerStatus {
            name: "Squad HQ".into(),
            version: "0.1.0".into(),
            users_online: 5,
            max_users: 64,
            features: vec!["x".into()],
        };
        assert_eq!(decode_status(&encode_status(&s)).unwrap(), s);
        assert!(encode_status(&s).len() < MAX_STATUS_BYTES);
    }

    fn user(role: Option<RoleId>) -> UserInfo {
        UserInfo {
            id: 7,
            nickname: "vasya".into(),
            channel_id: 2,
            muted: true,
            deafened: false,
            role,
            registered: true,
            has_key: true,
            streams: vec![MediaKind::Screen],
            watching: Some(3),
        }
    }

    fn roundtrip_client(m: ClientMsg) {
        assert_eq!(ClientMsg::decode_wire(&m.encode_wire()).unwrap(), Some(m));
    }

    fn roundtrip_server(m: ServerMsg) {
        assert_eq!(ServerMsg::decode_wire(&m.encode_wire()).unwrap(), Some(m));
    }

    #[test]
    fn every_message_roundtrips() {
        let ban = BanInfo {
            nickname: Some("x".into()),
            ip: None,
            key: Some("AB:CD".into()),
            reason: "r".into(),
            created: 5,
            expires: Some(3605),
            by: Some("mod".into()),
        };
        let console_ban = BanInfo { reason: "c".into(), ..BanInfo::default() };
        for m in [
            ClientMsg::Hello {
                client_version: "0.1.0".into(),
                features: vec!["future-thing".into()],
                nickname: "Алекс".into(),
                password: "p".into(),
                capabilities: vec![MediaKind::Screen, MediaKind::ScreenAudio],
            },
            ClientMsg::JoinChannel { channel_id: 3 },
            ClientMsg::ChatSend { channel_id: 3, text: "hi".into() },
            ClientMsg::PrivateSend { to: 4, text: "pm".into() },
            ClientMsg::SetVoiceState { muted: true, deafened: true },
            ClientMsg::AdminSetup { code: "ABCD-EFGH".into() },
            ClientMsg::SetRole { user_id: 4, role: Some(2) },
            ClientMsg::SetRole { user_id: 4, role: None },
            ClientMsg::SetRegistered { user_id: 4, registered: false },
            ClientMsg::ListUsers,
            ClientMsg::SetRoleByKey { key: "K".into(), role: Some(3) },
            ClientMsg::UnregisterKey { key: "K".into() },
            ClientMsg::ListBans,
            ClientMsg::Unban { ban: ban.clone() },
            ClientMsg::Kick { user_id: 4, reason: "k".into() },
            ClientMsg::Ban { user_id: 4, reason: "b".into(), duration_secs: Some(3600) },
            ClientMsg::Ban { user_id: 4, reason: "b".into(), duration_secs: None },
            ClientMsg::StartStream { kind: MediaKind::Screen },
            ClientMsg::StopStream { kind: MediaKind::Screen },
            ClientMsg::WatchStream { user_id: Some(4) },
            ClientMsg::WatchStream { user_id: None },
            ClientMsg::RequestKeyframe { user_id: 4 },
            ClientMsg::CreateChannel { name: "n".into(), description: "d".into(), max_users: Some(5) },
            ClientMsg::EditChannel { channel_id: 2, name: "n".into(), description: String::new(), max_users: None },
            ClientMsg::DeleteChannel { channel_id: 2 },
            ClientMsg::CreateRole { role: Roles::defaults().list[2].clone() },
            ClientMsg::EditRole { role: Roles::defaults().list[1].clone() },
            ClientMsg::DeleteRole { role_id: 3 },
            ClientMsg::MoveRole { role_id: 3, index: 1 },
            ClientMsg::GetServerSettings,
            ClientMsg::SetPassword { password: "новый пароль".into() },
            ClientMsg::SetPassword { password: String::new() },
        ] {
            roundtrip_client(m);
        }
        let channel = ChannelInfo { id: 1, name: "General".into(), description: "d".into(), max_users: Some(0) };
        let chat = ChatMessage {
            id: 1,
            channel_id: 1,
            author_id: 7,
            author_name: "v".into(),
            text: "t".into(),
            timestamp: -1,
        };
        let panel = PanelUser {
            key: None,
            nickname: "c".into(),
            role: Some(crate::OWNER_ROLE),
            registered: false,
            online: Some(3),
            created: 0,
            client_version: "0.1.0".into(),
        };
        for m in [
            ServerMsg::Welcome {
                your_id: 7,
                server_name: "S".into(),
                motd: "m".into(),
                channels: vec![channel.clone()],
                users: vec![user(Some(crate::OWNER_ROLE)), user(None)],
                admin_setup: true,
                server_version: "0.1.0".into(),
                features: vec![],
                roles: Roles::defaults(),
            },
            ServerMsg::Rejected { reason: RejectReason::WrongPassword },
            ServerMsg::Rejected { reason: RejectReason::Banned { reason: "spam".into(), remaining_secs: Some(90) } },
            ServerMsg::Rejected { reason: RejectReason::Banned { reason: "spam".into(), remaining_secs: None } },
            ServerMsg::Rejected { reason: RejectReason::ServerFull },
            ServerMsg::UserJoined(user(Some(3))),
            ServerMsg::UserLeft { user_id: 7 },
            ServerMsg::UserUpdated(user(Some(2))),
            ServerMsg::ChannelHistory { channel_id: 1, messages: vec![chat.clone()] },
            ServerMsg::Chat(chat),
            ServerMsg::Private(PrivateMessage {
                from_id: 1,
                from_name: "a".into(),
                to_id: 2,
                to_name: "b".into(),
                text: "t".into(),
                timestamp: 3,
            }),
            ServerMsg::AdminSetupResult { ok: true },
            ServerMsg::Kicked { reason: "r".into(), banned: true, ban_secs: Some(86_400) },
            ServerMsg::Kicked { reason: "r".into(), banned: true, ban_secs: None },
            ServerMsg::Notice { text: "n".into() },
            ServerMsg::Error { code: ErrorCode::RateLimited },
            ServerMsg::KeyframeRequest,
            ServerMsg::ChannelAdded(channel.clone()),
            ServerMsg::ChannelUpdated(channel),
            ServerMsg::ChannelRemoved { channel_id: 1 },
            ServerMsg::ServerUsers { users: vec![panel] },
            ServerMsg::Bans { bans: vec![ban, console_ban] },
            ServerMsg::ServerSettings { password_set: true },
            ServerMsg::ServerSettings { password_set: false },
            ServerMsg::Roles(Roles::defaults()),
        ] {
            roundtrip_server(m);
        }
    }

    /// A newer peer adds a field (tag 50): this version ignores it.
    #[test]
    fn unknown_field_is_skipped() {
        #[derive(Clone, PartialEq, Message)]
        struct NewerChatSend {
            #[prost(uint32, tag = "1")]
            channel_id: u32,
            #[prost(string, tag = "2")]
            text: String,
            #[prost(string, tag = "50")]
            future: String,
        }
        #[derive(Clone, PartialEq, Message)]
        struct NewerClientMsg {
            #[prost(message, optional, tag = "3")]
            chat_send: Option<NewerChatSend>,
        }
        let bytes =
            NewerClientMsg { chat_send: Some(NewerChatSend { channel_id: 1, text: "hi".into(), future: "x".into() }) }
                .encode_to_vec();
        assert_eq!(
            ClientMsg::decode_wire(&bytes).unwrap(),
            Some(ClientMsg::ChatSend { channel_id: 1, text: "hi".into() })
        );
    }

    /// A message type from a newer peer (tag 99) decodes to "unknown".
    #[test]
    fn unknown_message_type_is_none() {
        #[derive(Clone, PartialEq, Message)]
        struct Future {
            #[prost(message, optional, tag = "99")]
            something: Option<WNotice>,
        }
        let bytes = Future { something: Some(WNotice { text: "x".into() }) }.encode_to_vec();
        assert_eq!(ServerMsg::decode_wire(&bytes).unwrap(), None);
        assert_eq!(ClientMsg::decode_wire(&bytes).unwrap(), None);
    }

    /// Unknown enum numbers: shown data degrades, requests become unknown.
    #[test]
    fn unknown_enum_values() {
        let mut u = WUser::from(&user(Some(2)));
        u.streams = vec![2, 77];
        let bytes = WServerMsg { msg: Some(WServer::UserUpdated(u)) }.encode_to_vec();
        let Some(ServerMsg::UserUpdated(info)) = ServerMsg::decode_wire(&bytes).unwrap() else { panic!() };
        assert_eq!(info.streams, vec![MediaKind::Screen], "an unknown stream kind is dropped");

        let start = WClientMsg { msg: Some(WClient::StartStream(WStream { kind: 77 })) }.encode_to_vec();
        assert_eq!(ClientMsg::decode_wire(&start).unwrap(), None, "an unknown stream kind is not started");

        let mut role = WRole::from(&Roles::defaults().list[2]);
        role.perms = u32::MAX;
        let bytes = WServerMsg { msg: Some(WServer::Roles(WRoles { roles: vec![role] })) }.encode_to_vec();
        let Some(ServerMsg::Roles(r)) = ServerMsg::decode_wire(&bytes).unwrap() else { panic!() };
        assert_eq!(r.list[0].perms, Perms::ALL, "unknown rights are dropped");

        let err = WServerMsg { msg: Some(WServer::Error(WError { code: 999 })) }.encode_to_vec();
        assert_eq!(ServerMsg::decode_wire(&err).unwrap(), Some(ServerMsg::Error { code: ErrorCode::Unknown }));
        let rej = WServerMsg {
            msg: Some(WServer::Rejected(WReject { code: 99, reason: String::new(), remaining_secs: None })),
        }
        .encode_to_vec();
        assert_eq!(ServerMsg::decode_wire(&rej).unwrap(), Some(ServerMsg::Rejected { reason: RejectReason::Other }));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(ServerMsg::decode_wire(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }
}
