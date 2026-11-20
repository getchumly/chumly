use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use shared::media::MediaKind;
use shared::{
    ChannelId, ChannelInfo, ChatMessage, ClientMsg, ErrorCode, OWNER_ROLE, Perms, PrivateMessage, RejectReason,
    ServerMsg, UserId, UserInfo,
};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{info, warn};

use crate::bans::{Ban, BanList};
use crate::config::ServerConfig;
use crate::limits::{AuthFailures, ConnCounter, Secret, secret_eq};
use crate::tls::short_key;
use crate::users::Registry;

// `impl State` is split by concern; this file keeps the core: connected
// users, login and logout, sending to them.
mod admin;
mod channels;
mod chat;
mod handle;
mod media;
mod roles;

use admin::new_setup_code;
pub use admin::{Actor, Punish};

/// Connections allowed to be in the handshake or login at the same time.
/// Logged-in users do not count.
const MAX_PENDING_LOGINS: usize = 64;

pub struct Server {
    pub state: Mutex<State>,
    /// Open connections per address.
    pub conns: ConnCounter,
    /// Slots for connections that have not logged in yet.
    pub pending: Arc<Semaphore>,
    /// When each address last got a status answer (see `conn::serve_status`).
    pub status_seen: std::sync::Mutex<HashMap<std::net::IpAddr, Instant>>,
}

impl Server {
    pub fn new(config: ServerConfig, bans: BanList, registry: Registry) -> Self {
        Self {
            state: Mutex::new(State::new(config, bans, registry)),
            conns: ConnCounter::default(),
            status_seen: std::sync::Mutex::new(HashMap::new()),
            pending: Arc::new(Semaphore::new(MAX_PENDING_LOGINS)),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic in one handler must not bring down the whole server.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Connected {
    pub info: UserInfo,
    pub addr: SocketAddr,
    pub tx: Sender<ServerMsg>,
    pub conn: quinn::Connection,
    /// Public key fingerprint: who this is, independent of the nickname.
    /// `None` for clients without a key (unreadable identity file).
    pub key: Option<String>,
    /// The client can display screen shares.
    pub can_view: bool,
    /// Client version from `Hello`, for the admin panel and the console.
    pub client_version: String,
}

impl Connected {
    /// Queues a message for the writer task. A client that does not read
    /// fast enough would make the queue grow without bound, so when it is
    /// full the client is disconnected instead.
    fn push(&self, msg: ServerMsg) {
        match self.tx.try_send(msg) {
            Ok(()) | Err(TrySendError::Closed(_)) => {}
            Err(TrySendError::Full(_)) => {
                warn!("{} ({}): send queue full, disconnecting", self.info.nickname, self.addr);
                self.conn.close(1u32.into(), b"too slow");
            }
        }
    }
}

pub struct State {
    pub config: ServerConfig,
    pub channels: Vec<ChannelInfo>,
    pub users: HashMap<UserId, Connected>,
    history: HashMap<ChannelId, VecDeque<ChatMessage>>,
    pub bans: BanList,
    /// Wrong password attempts per address.
    pub auth: AuthFailures,
    /// Registered nicknames, roles by key and the role list (users.toml).
    pub registry: Registry,
    /// One-time code that makes the first admin. Exists while the
    /// registry has no admin; printed to the log at startup.
    pub setup_code: Option<String>,
    next_user_id: UserId,
    next_msg_id: u64,
    /// Channel ids are not reused while the server is running:
    /// clients may still hold references to a deleted channel.
    next_channel_id: ChannelId,
}

pub struct Hello {
    /// Client version, for the log.
    pub client_version: String,
    pub nickname: String,
    pub password: String,
    pub can_view: bool,
    /// Public key fingerprint from the client certificate, if any.
    pub key: Option<String>,
}

/// What the caller should do after handling a message.
#[derive(PartialEq, Eq)]
pub enum After {
    Nothing,
    /// Wrong admin password: slow the client down
    /// so the password cannot be brute-forced quickly.
    Penalty,
}

impl State {
    fn new(config: ServerConfig, bans: BanList, registry: Registry) -> Self {
        let channels = config
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| ChannelInfo {
                id: i as ChannelId + 1,
                name: c.name.clone(),
                description: c.description.clone(),
                max_users: c.max_users,
            })
            .collect::<Vec<_>>();
        Self {
            next_channel_id: channels.len() as ChannelId + 1,
            config,
            channels,
            users: HashMap::new(),
            history: HashMap::new(),
            bans,
            auth: AuthFailures::new(Instant::now()),
            setup_code: registry.owner().is_none().then(new_setup_code),
            registry,
            next_user_id: 1,
            next_msg_id: 1,
        }
    }

    pub fn find_by_nick(&self, nick: &str) -> Option<UserId> {
        let nick = nick.to_lowercase();
        self.users.values().find(|u| u.info.nickname.to_lowercase() == nick).map(|u| u.info.id)
    }

    fn send(&self, id: UserId, msg: ServerMsg) {
        if let Some(u) = self.users.get(&id) {
            u.push(msg);
        }
    }

    fn broadcast(&self, msg: &ServerMsg, except: Option<UserId>) {
        for u in self.users.values() {
            if Some(u.info.id) != except {
                u.push(msg.clone());
            }
        }
    }

    pub fn register(
        &mut self,
        hello: Hello,
        addr: SocketAddr,
        tx: Sender<ServerMsg>,
        conn: quinn::Connection,
    ) -> Result<UserId, RejectReason> {
        if !self.config.password.is_empty() {
            let now = Instant::now();
            // A blocked address (or a stranger while the server is under
            // attack) gets the same answer as a wrong password, so guessing
            // cannot tell when the right one was sent.
            if !self.auth.may_check(addr.ip(), Secret::Server, now) {
                return Err(RejectReason::WrongPassword);
            }
            if !secret_eq(&hello.password, &self.config.password) {
                if self.auth.record_failure(addr.ip(), now) {
                    warn!("{}: too many wrong server passwords, address blocked", addr.ip());
                }
                return Err(RejectReason::WrongPassword);
            }
            self.auth.record_success(addr.ip(), Secret::Server);
        }
        if !shared::validate_nickname(&hello.nickname) {
            return Err(RejectReason::InvalidNickname);
        }
        let now = shared::unix_now();
        if let Some(ban) = self.bans.find_at(&hello.nickname, addr.ip(), hello.key.as_deref(), now) {
            return Err(RejectReason::Banned { reason: ban.reason.clone(), remaining_secs: ban.remaining_at(now) });
        }
        let owner = self.registry.by_nick(&hello.nickname).map(|r| r.key.clone());
        if owner.is_some() && owner.as_deref() != hello.key.as_deref() {
            return Err(RejectReason::NicknameReserved);
        }
        if self.find_by_nick(&hello.nickname).is_some() {
            return Err(RejectReason::NicknameTaken);
        }
        if self.users.len() >= self.config.max_users as usize {
            return Err(RejectReason::ServerFull);
        }
        let channel_id =
            self.channels.iter().find(|c| self.channel_has_room(c)).map(|c| c.id).ok_or(RejectReason::ServerFull)?;

        // A registered user who connects under a new, free nickname renames
        // their registration: the role is bound to the key, not the name.
        let mut role = None;
        let mut registered = false;
        if let Some(key) = hello.key.as_deref()
            && let Some(old) = self.registry.by_key(key).map(|r| r.nickname.clone())
        {
            if old != hello.nickname {
                info!("registered user renamed: {old} -> {}", hello.nickname);
                self.registry.register(key, &hello.nickname);
                self.registry.save();
            }
            role = self.registry.by_key(key).and_then(|r| r.role);
            registered = true;
        }

        let id = self.next_user_id;
        self.next_user_id += 1;
        let info = UserInfo {
            id,
            nickname: hello.nickname,
            channel_id,
            muted: false,
            deafened: false,
            role,
            registered,
            has_key: hello.key.is_some(),
            streams: Vec::new(),
            watching: None,
        };
        info!(
            "join: {} from {} key {} into channel '{}' (client {})",
            info.nickname,
            addr,
            hello.key.as_deref().map_or("-", short_key),
            self.channel_name(channel_id),
            if hello.client_version.is_empty() { "?" } else { &hello.client_version }
        );
        self.broadcast(&ServerMsg::UserJoined(info.clone()), None);
        self.users.insert(
            id,
            Connected {
                info,
                addr,
                tx,
                conn,
                key: hello.key,
                can_view: hello.can_view,
                client_version: hello.client_version,
            },
        );

        let welcome = ServerMsg::Welcome {
            your_id: id,
            server_name: self.config.name.clone(),
            motd: self.config.motd.clone(),
            channels: self.channels.clone(),
            users: self.users.values().map(|u| u.info.clone()).collect(),
            admin_setup: self.setup_code.is_some(),
            server_version: shared::VERSION.to_string(),
            features: shared::feature_names(),
            roles: self.registry.roles.clone(),
        };
        self.send(id, welcome);
        self.send_history(id, channel_id);
        Ok(id)
    }

    pub fn unregister(&mut self, id: UserId) {
        if let Some(u) = self.users.remove(&id) {
            info!("leave: {}", u.info.nickname);
            self.broadcast(&ServerMsg::UserLeft { user_id: id }, None);
            self.drop_viewers(id);
        }
    }

    fn update_user(&mut self, id: UserId, f: impl FnOnce(&mut UserInfo)) {
        let Some(u) = self.users.get_mut(&id) else { return };
        f(&mut u.info);
        let msg = ServerMsg::UserUpdated(u.info.clone());
        self.broadcast(&msg, None);
    }

    pub fn send_error(&self, id: UserId, code: ErrorCode) {
        self.send(id, ServerMsg::Error { code });
    }

    pub fn notice(&self, text: &str) {
        self.broadcast(&ServerMsg::Notice { text: text.to_string() }, None);
    }
}
