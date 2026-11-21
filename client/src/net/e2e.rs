//! End-to-end tests against a real server. Ignored by default. To run:
//! 1. start `chumly-server --config <dir>/server.toml` with
//!    `bind = "127.0.0.1:17777"` and `password = "secret"`;
//! 2. `cargo test -p client -- --ignored end_to_end screen_stress --nocapture`.
//!
//! On a fresh server (no users.toml) the first run needs the admin setup code
//! from the server log: `CHUMLY_SETUP_CODE=XXXX-XXXX cargo test ...`.
//! Or all of it in one go: `pwsh .github/scripts/e2e.ps1` (what CI runs).

use std::sync::mpsc::Receiver;
use std::time::Instant;

use shared::{ErrorCode, OWNER_ROLE, RoleId, UserId, UserInfo};

use crate::audio::lock_mixer;

/// Ids of the default roles (`shared::Roles::defaults`); the test server
/// keeps them.
const ADMIN: RoleId = 2;
const MODERATOR: RoleId = 3;

use super::*;

struct TestClient {
    cmds: mpsc::UnboundedSender<NetCommand>,
    events: Receiver<NetEvent>,
    voice: mpsc::Sender<VoiceOut>,
    #[cfg_attr(not(windows), allow(dead_code))]
    screen_tx: mpsc::Sender<(MediaKind, Vec<u8>)>,
    #[cfg_attr(not(windows), allow(dead_code))]
    screen: Arc<crate::screen::ScreenShared>,
    mixer: Arc<Mutex<Mixer>>,
    identity: Option<Arc<Identity>>,
}

fn addr() -> String {
    std::env::var("CHUMLY_TEST_ADDR").unwrap_or_else(|_| "127.0.0.1:17777".into())
}

/// A client with its own fresh identity.
fn client(rt: &tokio::runtime::Runtime) -> TestClient {
    client_with(rt, Some(Arc::new(Identity::generate().unwrap())))
}

/// `None`: a client without a key (unreadable identity file).
fn client_with(rt: &tokio::runtime::Runtime, identity: Option<Arc<Identity>>) -> TestClient {
    let (ev_tx, ev_rx) = std::sync::mpsc::channel();
    let (voice, voice_rx) = mpsc::channel(64);
    let mixer = Arc::new(Mutex::new(Mixer::default()));
    let shared = Arc::new(AudioShared::new(&crate::config::AudioConfig::default()));
    let (screen_tx, screen_rx) = mpsc::channel(1024);
    let screen = Arc::new(crate::screen::ScreenShared::default());
    let cmds =
        spawn(rt, eframe::egui::Context::default(), ev_tx, voice_rx, screen_rx, mixer.clone(), shared, screen.clone());
    TestClient { cmds, events: ev_rx, voice, screen_tx, screen, mixer, identity }
}

impl TestClient {
    fn connect(&self, nickname: &str, password: &str, fingerprint: Option<String>) {
        let p = ConnectParams {
            address: addr(),
            nickname: nickname.into(),
            password: password.into(),
            fingerprint,
            identity: self.identity.clone(),
        };
        self.cmds.send(NetCommand::Connect(p)).unwrap();
    }

    fn send(&self, m: ClientMsg) {
        self.cmds.send(NetCommand::Send(m)).unwrap();
    }

    /// Waits for an event for which `f` returns `Some`. Skips the rest.
    fn wait<T>(&self, what: &str, mut f: impl FnMut(&NetEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = self.events.recv_timeout(left).unwrap_or_else(|_| panic!("timed out waiting for: {what}"));
            if let Some(v) = f(&ev) {
                return v;
            }
        }
    }

    fn welcome(&self) -> (UserId, shared::ChannelId) {
        self.wait("Welcome", |e| match e {
            NetEvent::Message(ServerMsg::Welcome { your_id, users, .. }) => {
                Some((*your_id, users.iter().find(|u| u.id == *your_id).unwrap().channel_id))
            }
            _ => None,
        })
    }
}

/// The server password from the owner's panel: only the owner may see or
/// change it; a new password works at once, the old one stops working;
/// without one anyone joins. Ends with the test password "secret" again.
fn server_password(a: &TestClient, b: &TestClient, c: &TestClient, fp: &str) {
    let settings = |cl: &TestClient, what: &str, want: bool| {
        cl.wait(what, |e| {
            matches!(e, NetEvent::Message(ServerMsg::ServerSettings { password_set }) if *password_set == want)
                .then_some(())
        })
    };
    let refused = |cl: &TestClient, what: &str, code: ErrorCode| {
        cl.wait(what, |e| matches!(e, NetEvent::Message(ServerMsg::Error { code: c }) if *c == code).then_some(()))
    };
    // bob is not the owner, whatever rights his role has.
    b.send(ClientMsg::GetServerSettings);
    refused(b, "settings refused to bob", ErrorCode::NotAdmin);
    b.send(ClientMsg::SetPassword { password: "bob".into() });
    refused(b, "password refused to bob", ErrorCode::NotAdmin);

    a.send(ClientMsg::GetServerSettings);
    settings(a, "password is set", true);
    a.send(ClientMsg::SetPassword { password: " spaces ".into() });
    refused(a, "bad password", ErrorCode::InvalidPassword);

    a.send(ClientMsg::SetPassword { password: "новый пароль".into() });
    settings(a, "password changed", true);
    c.connect("carol", "secret", Some(fp.to_string()));
    c.wait("old password refused", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::Rejected(RejectReason::WrongPassword))).then_some(())
    });
    c.connect("carol", "новый пароль", Some(fp.to_string()));
    c.welcome();
    c.cmds.send(NetCommand::Disconnect).unwrap();
    c.wait("carol out", |e| matches!(e, NetEvent::Disconnected(DisconnectReason::User)).then_some(()));

    a.send(ClientMsg::SetPassword { password: String::new() });
    settings(a, "password removed", false);
    c.connect("carol", "anything", Some(fp.to_string()));
    c.welcome();
    c.cmds.send(NetCommand::Disconnect).unwrap();
    c.wait("carol out again", |e| matches!(e, NetEvent::Disconnected(DisconnectReason::User)).then_some(()));

    a.send(ClientMsg::SetPassword { password: "secret".into() });
    settings(a, "test password back", true);
}

/// Roles and registration. alice is the owner. Returns bob's new id
/// (he reconnects under another nickname).
fn roles(a: &TestClient, alice: UserId, b: &TestClient, bob: UserId, c: &TestClient, fp: &str) -> UserId {
    let updated = |cl: &TestClient, what: &str, f: &dyn Fn(&UserInfo) -> bool| {
        cl.wait(what, |e| matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if f(u)).then_some(()))
    };
    let error = |cl: &TestClient, code: ErrorCode| {
        cl.wait(&format!("{code:?}"), |e| {
            matches!(e, NetEvent::Message(ServerMsg::Error { code: c }) if *c == code).then_some(())
        })
    };

    // The registered admin's nickname is reserved, even in another case.
    c.connect("ALICE", "secret", Some(fp.to_string()));
    c.wait("nickname reserved", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::Rejected(RejectReason::NicknameReserved))).then_some(())
    });
    c.connect("carol", "secret", Some(fp.to_string()));
    let (carol, _) = c.welcome();

    // Nobody changes their own role, the owner included; clients without a
    // key cannot get one.
    a.send(ClientMsg::SetRole { user_id: alice, role: None });
    error(a, ErrorCode::CannotTargetSelf);
    a.send(ClientMsg::SetRole { user_id: carol, role: Some(MODERATOR) });
    error(a, ErrorCode::IdentityRequired);

    a.send(ClientMsg::SetRole { user_id: bob, role: Some(MODERATOR) });
    updated(b, "bob is moderator", &|u| u.id == bob && u.role == Some(MODERATOR) && u.registered);

    // A moderator kicks users without a role, but not the admin.
    b.send(ClientMsg::Kick { user_id: alice, reason: String::new() });
    error(b, ErrorCode::NotAdmin);
    b.send(ClientMsg::Ban { user_id: carol, reason: String::new(), duration_secs: None });
    error(b, ErrorCode::NotAdmin);
    b.send(ClientMsg::Kick { user_id: carol, reason: "mod".into() });
    c.wait("carol kicked", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::Kicked { reason, banned: false, .. }) if reason == "mod")
            .then_some(())
    });

    // Admin panel requests are refused by the server for a moderator,
    // whatever the client UI shows.
    let fake_ban = shared::BanInfo::default();
    for m in [
        ClientMsg::ListUsers,
        ClientMsg::ListBans,
        ClientMsg::SetRoleByKey { key: "AB".into(), role: Some(ADMIN) },
        ClientMsg::UnregisterKey { key: "AB".into() },
        ClientMsg::Unban { ban: fake_ban },
    ] {
        b.send(m);
        error(b, ErrorCode::NotAdmin);
    }

    // The admin sees registered users, online or not, with their keys. Earlier
    // lists may still be queued (the server sends one after every role change,
    // even a refused one): wait for the state we expect instead of asserting
    // on the first list.
    a.send(ClientMsg::ListUsers);
    let (bob_key, alice_key) = a.wait("server users", |e| match e {
        NetEvent::Message(ServerMsg::ServerUsers { users }) => {
            let al = users.iter().find(|u| u.nickname == "alice")?;
            let bo = users.iter().find(|u| u.nickname == "bob")?;
            let ok = al.online == Some(alice)
                && al.registered
                && al.role == Some(OWNER_ROLE)
                && bo.online == Some(bob)
                && bo.registered
                && bo.role == Some(MODERATOR);
            if ok { Some((bo.key.clone()?, al.key.clone()?)) } else { None }
        }
        _ => None,
    });

    // Under a new nickname with the same key: the registration follows. While bob
    // is offline the admin changes his role by key; he gets it on the next login.
    b.cmds.send(NetCommand::Disconnect).unwrap();
    b.wait("bob disconnected", |e| matches!(e, NetEvent::Disconnected(DisconnectReason::User)).then_some(()));
    a.send(ClientMsg::SetRoleByKey { key: bob_key.clone(), role: Some(ADMIN) });
    a.wait("list after role change", |e| match e {
        NetEvent::Message(ServerMsg::ServerUsers { users }) => {
            let bo = users.iter().find(|u| u.key.as_deref() == Some(bob_key.as_str()))?;
            (bo.role == Some(ADMIN) && bo.online.is_none()).then_some(())
        }
        _ => None,
    });
    b.connect("bobby", "secret", Some(fp.to_string()));
    let bobby = b.wait("Welcome", |e| match e {
        NetEvent::Message(ServerMsg::Welcome { your_id, users, admin_setup, .. }) => {
            assert!(!admin_setup);
            let me = users.iter().find(|u| u.id == *your_id).unwrap();
            assert_eq!(me.role, Some(ADMIN));
            assert!(me.registered);
            Some(*your_id)
        }
        _ => None,
    });

    // An admin acts only on those below: not on the owner, and gives only
    // roles below his own.
    b.send(ClientMsg::Kick { user_id: alice, reason: String::new() });
    error(b, ErrorCode::NotAdmin);
    b.send(ClientMsg::SetRoleByKey { key: alice_key, role: None });
    error(b, ErrorCode::NotAdmin);
    b.send(ClientMsg::SetRoleByKey { key: bob_key.clone(), role: Some(OWNER_ROLE) });
    error(b, ErrorCode::CannotTargetSelf);

    // Roles are data: the owner creates one at the bottom and everyone gets
    // the new list; an admin edits only roles below his own. A unique name,
    // so a stopped run does not break the next one.
    let name = format!("e2e {}", shared::unix_now());
    let helper = shared::RoleInfo {
        id: 0,
        name: name.clone(),
        color: "teal".into(),
        icon: "star".into(),
        perms: shared::Perms::KICK,
    };
    a.send(ClientMsg::CreateRole { role: helper });
    let roles = b.wait("roles with the new one", |e| match e {
        NetEvent::Message(ServerMsg::Roles(r)) if r.list.last().is_some_and(|x| x.name == name) => Some(r.clone()),
        _ => None,
    });
    let helper_id = roles.list.last().unwrap().id;
    let mut admin_role = roles.get(ADMIN).expect("the default admin role").clone();
    admin_role.name = "Boss".into();
    b.send(ClientMsg::EditRole { role: admin_role });
    error(b, ErrorCode::NotAdmin);
    b.send(ClientMsg::MoveRole { role_id: helper_id, index: 1 });
    error(b, ErrorCode::NotAdmin);
    b.send(ClientMsg::DeleteRole { role_id: helper_id });
    a.wait("roles without it", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Roles(r)) if r.get(helper_id).is_none()).then_some(())
    });

    // Ban and unban through the admin panel. Note: the ban covers 127.0.0.1;
    // if the test stops in between, run `unban 127.0.0.1` in the server console.
    c.connect("carol", "secret", Some(fp.to_string()));
    let (carol, _) = c.welcome();
    // A temporary ban: the user is told its length, a later attempt the
    // time left; the panel shows its end and who issued it.
    a.send(ClientMsg::Ban { user_id: carol, reason: "e2e".into(), duration_secs: Some(3600) });
    c.wait("carol banned", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::Kicked { banned: true, ban_secs: Some(3600), .. }))
            .then_some(())
    });
    c.connect("carol", "secret", Some(fp.to_string()));
    c.wait("ban works", |e| {
        matches!(
            e,
            NetEvent::Disconnected(DisconnectReason::Rejected(RejectReason::Banned { remaining_secs: Some(s), .. }))
                if (3590..=3600).contains(s)
        )
        .then_some(())
    });
    a.send(ClientMsg::ListBans);
    let ban = a.wait("bans", |e| match e {
        NetEvent::Message(ServerMsg::Bans { bans }) => bans.iter().find(|b| b.reason == "e2e").cloned(),
        _ => None,
    });
    assert_eq!(ban.by.as_deref(), Some("alice"));
    let left = ban.expires.expect("temporary") - shared::unix_now();
    assert!((3590..=3600).contains(&left), "{left}");
    a.send(ClientMsg::Unban { ban: ban.clone() });
    a.wait("ban lifted", |e| match e {
        NetEvent::Message(ServerMsg::Bans { bans }) => (!bans.contains(&ban)).then_some(()),
        _ => None,
    });
    c.connect("carol", "secret", Some(fp.to_string()));
    let (carol, _) = c.welcome();

    // Clean up so the test server does not collect registrations.
    a.send(ClientMsg::SetRegistered { user_id: bobby, registered: false });
    updated(b, "bobby unregistered", &|u| u.id == bobby && u.role.is_none() && !u.registered);

    // The panel also lists online users who are not registered: with a key
    // (bobby now) and without one (carol).
    a.send(ClientMsg::ListUsers);
    a.wait("unregistered online users listed", |e| match e {
        NetEvent::Message(ServerMsg::ServerUsers { users }) => {
            let by = users.iter().find(|u| u.nickname == "bobby" && !u.registered)?;
            let ca = users.iter().find(|u| u.nickname == "carol")?;
            assert!(by.key.is_some() && by.online == Some(bobby) && by.created == 0);
            assert!(!ca.registered && ca.key.is_none() && ca.online == Some(carol));
            assert!(users.first().is_some_and(|u| u.registered), "registered users come first");
            Some(())
        }
        _ => None,
    });
    c.cmds.send(NetCommand::Disconnect).unwrap();
    bobby
}

#[test]
#[ignore = "requires a running server"]
fn end_to_end() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    // alice's identity is fixed: after the first run she stays the owner of
    // the test server, and later runs need no setup code.
    let a = client_with(&rt, Some(Arc::new(Identity::from_secret([7; 32]))));
    let b = client(&rt);
    // Without a key (unreadable identity file): still let in.
    let c = client_with(&rt, None);

    c.connect("carol", "wrong", None);
    c.wait("wrong password rejection", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::Rejected(RejectReason::WrongPassword))).then_some(())
    });

    a.connect("alice", "secret", None);
    let fp = a.wait("Connected", |e| match e {
        NetEvent::Connected { fingerprint, .. } => Some(fingerprint.clone()),
        _ => None,
    });
    assert_eq!(fp.len(), 95, "SHA-256 as XX:XX:...");
    let (alice, ch, alice_admin, admin_setup) = a.wait("Welcome", |e| match e {
        NetEvent::Message(ServerMsg::Welcome { your_id, users, admin_setup, .. }) => {
            let me = users.iter().find(|u| u.id == *your_id).unwrap();
            assert!(me.has_key);
            Some((*your_id, me.channel_id, me.role == Some(OWNER_ROLE), *admin_setup))
        }
        _ => None,
    });
    assert_eq!(admin_setup, !alice_admin, "setup is offered exactly while there is no admin");

    // The password is checked before the nickname: a wrong password with a
    // taken nickname says "wrong password", and reveals nothing about who is
    // online.
    c.connect("alice", "wrong", None);
    c.wait("wrong password before taken nickname", |e| match e {
        NetEvent::Disconnected(DisconnectReason::Rejected(r)) => {
            assert_eq!(*r, RejectReason::WrongPassword);
            Some(())
        }
        _ => None,
    });

    c.connect("carol", "secret", Some("00:11".into()));
    c.wait("certificate mismatch", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::CertMismatch { .. })).then_some(())
    });

    b.connect("bob", "secret", Some(fp.clone()));
    let (bob, bob_ch) = b.welcome();
    assert_eq!(bob_ch, ch);
    a.wait("bob joined", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserJoined(u)) if u.nickname == "bob").then_some(())
    });

    c.connect("BOB", "secret", Some(fp.clone()));
    c.wait("nickname taken", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::Rejected(RejectReason::NicknameTaken))).then_some(())
    });

    a.send(ClientMsg::ChatSend { channel_id: ch, text: "hello".into() });
    b.wait("channel message", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Chat(m)) if m.text == "hello" && m.author_id == alice).then_some(())
    });

    b.send(ClientMsg::PrivateSend { to: alice, text: "private".into() });
    a.wait("private message", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Private(pm)) if pm.text == "private" && pm.from_id == bob)
            .then_some(())
    });

    // Voice: alice sends 10 frames, bob must hear them.
    let mut enc = opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
    for seq in 0..10u32 {
        let pcm: Vec<f32> = (0..960).map(|i| ((seq * 960 + i) as f32 * 0.06).sin() * 0.4).collect();
        let mut buf = [0u8; 1500];
        let len = enc.encode_float(&pcm, &mut buf).unwrap();
        a.voice.blocking_send(VoiceOut { seq, data: buf[..len].to_vec() }).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while !lock_mixer(&b.mixer).is_speaking(alice) {
        assert!(Instant::now() < deadline, "alice's voice did not reach bob");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = vec![0.0f32; 4800];
    lock_mixer(&b.mixer).pull(&mut out);
    assert!(out.iter().any(|s| s.abs() > 0.01), "decoded voice is not silence");

    #[cfg(windows)]
    screen_share(&a, alice, &b, bob);

    b.send(ClientMsg::Kick { user_id: alice, reason: String::new() });
    b.wait("kick without permission", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Error { code: ErrorCode::NotAdmin })).then_some(())
    });

    a.send(ClientMsg::AdminSetup { code: "BAD0-CODE".into() });
    a.wait("wrong setup code", |e| {
        matches!(e, NetEvent::Message(ServerMsg::AdminSetupResult { ok: false })).then_some(())
    });
    if !alice_admin {
        let code = std::env::var("CHUMLY_SETUP_CODE")
            .expect("fresh test server: set CHUMLY_SETUP_CODE to the admin setup code from the server log");
        a.send(ClientMsg::AdminSetup { code });
        a.wait("admin setup", |e| {
            matches!(e, NetEvent::Message(ServerMsg::AdminSetupResult { ok: true })).then_some(())
        });
        b.wait("alice is owner", |e| {
            matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == alice && u.role == Some(OWNER_ROLE) && u.registered)
                .then_some(())
        });
    }

    channel_admin(&a, alice, &b, bob, ch);
    let bob = roles(&a, alice, &b, bob, &c, &fp);

    a.send(ClientMsg::JoinChannel { channel_id: ch + 1 });
    b.wait("alice switched channel", |e| {
        // Switching channels also stops the screen share.
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u))
            if u.id == alice && u.channel_id == ch + 1 && u.streams.is_empty())
        .then_some(())
    });

    // Channel switches are limited (burst of 5, then 1 per second): a burst of
    // switches is refused with RateLimited, without disconnecting.
    for i in 0..8 {
        a.send(ClientMsg::JoinChannel { channel_id: if i % 2 == 0 { ch } else { ch + 1 } });
    }
    a.wait("join rate limit", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Error { code: ErrorCode::RateLimited })).then_some(())
    });
    a.send(ClientMsg::ListBans);
    a.wait("still connected", |e| matches!(e, NetEvent::Message(ServerMsg::Bans { .. })).then_some(()));

    server_password(&a, &b, &c, &fp);

    a.send(ClientMsg::Kick { user_id: bob, reason: "test".into() });
    b.wait("bob kicked", |e| {
        matches!(e, NetEvent::Disconnected(DisconnectReason::Kicked { reason, banned: false, .. }) if reason == "test")
            .then_some(())
    });
    a.wait("bob left", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserLeft { user_id }) if *user_id == bob).then_some(())
    });

    a.cmds.send(NetCommand::Disconnect).unwrap();
    a.wait("disconnect", |e| matches!(e, NetEvent::Disconnected(DisconnectReason::User)).then_some(()));
}

/// Admin alice creates, edits and deletes a channel; bob sees
/// the changes and hits the limit.
fn channel_admin(a: &TestClient, alice: UserId, b: &TestClient, bob: UserId, home: shared::ChannelId) {
    b.send(ClientMsg::CreateChannel { name: "Foreign".into(), description: String::new(), max_users: None });
    b.wait("create channel without permission", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Error { code: ErrorCode::NotAdmin })).then_some(())
    });

    a.send(ClientMsg::CreateChannel { name: " Test ".into(), description: "e2e".into(), max_users: Some(1) });
    let new = b.wait("channel created", |e| match e {
        NetEvent::Message(ServerMsg::ChannelAdded(c)) if c.name == "Test" && c.max_users == Some(1) => Some(c.id),
        _ => None,
    });

    a.send(ClientMsg::CreateChannel { name: "test".into(), description: String::new(), max_users: None });
    a.wait("name taken", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Error { code: ErrorCode::ChannelNameTaken })).then_some(())
    });

    a.send(ClientMsg::JoinChannel { channel_id: new });
    b.wait("alice in the new channel", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == alice && u.channel_id == new).then_some(())
    });
    b.send(ClientMsg::JoinChannel { channel_id: new });
    b.wait("channel limit", |e| {
        matches!(e, NetEvent::Message(ServerMsg::Error { code: ErrorCode::ChannelFull })).then_some(())
    });

    a.send(ClientMsg::EditChannel {
        channel_id: new,
        name: "Test 2".into(),
        description: String::new(),
        max_users: None,
    });
    b.wait("channel renamed", |e| {
        matches!(e, NetEvent::Message(ServerMsg::ChannelUpdated(c))
            if c.id == new && c.name == "Test 2" && c.max_users.is_none())
        .then_some(())
    });
    b.send(ClientMsg::JoinChannel { channel_id: new });
    a.wait("bob joined after limit removed", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == bob && u.channel_id == new).then_some(())
    });

    a.send(ClientMsg::DeleteChannel { channel_id: new });
    b.wait("bob moved out of the deleted channel", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == bob && u.channel_id == home).then_some(())
    });
    b.wait("channel deleted", |e| {
        matches!(e, NetEvent::Message(ServerMsg::ChannelRemoved { channel_id }) if *channel_id == new).then_some(())
    });
}

/// Screen share: alice shares, bob watches (and alice sees him among her
/// viewers). A frame is compressed by the real encoder, split into fragments
/// and passed through the server.
#[cfg(windows)]
fn screen_share(a: &TestClient, alice: UserId, b: &TestClient, bob: UserId) {
    use openh264::encoder::{Encoder, EncoderConfig, UsageType};
    use openh264::formats::{BgraSliceU8, YUVBuffer};
    use shared::media::{SCREEN_FRAGMENT_DATA, ScreenFragment};

    a.send(ClientMsg::StartStream { kind: MediaKind::Screen });
    b.wait("alice is live", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == alice && u.streams.contains(&MediaKind::Screen))
            .then_some(())
    });

    let viewer =
        crate::screen::Viewer::start(alice, b.screen.clone(), b.cmds.clone(), eframe::egui::Context::default())
            .unwrap();
    b.send(ClientMsg::WatchStream { user_id: Some(alice) });
    a.wait("bob among alice's viewers", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == bob && u.watching == Some(alice))
            .then_some(())
    });
    a.wait("keyframe request from new viewer", |e| {
        matches!(e, NetEvent::Message(ServerMsg::KeyframeRequest)).then_some(())
    });

    // Noise compresses poorly: the keyframe will take many datagrams.
    let (w, h) = (640usize, 480usize);
    let mut x: u32 = 1;
    let bgra: Vec<u8> = (0..w * h * 4)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    let config = EncoderConfig::new().usage_type(UsageType::ScreenContentRealTime);
    let mut enc = Encoder::with_api_config(openh264::OpenH264API::from_source(), config).unwrap();
    let mut yuv = YUVBuffer::new(w, h);
    yuv.read_bgra8(BgraSliceU8::new(&bgra, (w, h)));
    let data = enc.encode(&yuv).unwrap().to_vec();
    let count = data.len().div_ceil(SCREEN_FRAGMENT_DATA);
    assert!(count > 5, "frame should span several datagrams, but took {count}");
    for (i, chunk) in data.chunks(SCREEN_FRAGMENT_DATA).enumerate() {
        let f = ScreenFragment { frame_id: 0, index: i as u16, count: count as u16, keyframe: true, data: chunk };
        a.screen_tx.blocking_send((MediaKind::Screen, f.encode())).unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    let image = loop {
        if let Some(img) = viewer.take_frame() {
            break img;
        }
        assert!(Instant::now() < deadline, "screen share frame did not reach bob");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(image.size, [w, h]);

    // Screen share audio reaches the viewer's mixer.
    let mut enc = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
    for seq in 0..8u32 {
        let pcm: Vec<f32> = (0..960)
            .flat_map(|i| {
                let v = ((seq * 960 + i) as f32 * 0.05).sin() * 0.4;
                [v, v]
            })
            .collect();
        let mut buf = [0u8; 4000];
        let len = enc.encode_float(&pcm, &mut buf).unwrap();
        a.screen_tx.blocking_send((MediaKind::ScreenAudio, buf[..len].to_vec())).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut out = vec![0.0f32; 960 * 2];
    loop {
        lock_mixer(&b.mixer).pull(&mut out);
        if out.iter().any(|s| s.abs() > 0.01) {
            break;
        }
        assert!(Instant::now() < deadline, "screen share audio did not reach bob");
        std::thread::sleep(Duration::from_millis(20));
    }

    b.send(ClientMsg::RequestKeyframe { user_id: alice });
    a.wait("keyframe request from viewer", |e| {
        matches!(e, NetEvent::Message(ServerMsg::KeyframeRequest)).then_some(())
    });

    // The share ends: its viewers stop watching, and everyone sees it.
    a.send(ClientMsg::StopStream { kind: MediaKind::Screen });
    a.wait("bob no longer watching", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == bob && u.watching.is_none()).then_some(())
    });
}

/// Load similar to the heaviest share, 1080p 60 at 12 Mbit/s: 22 KB frames
/// and a 300 KB keyframe every second, about 13 Mbit/s. Counts how much
/// reaches the viewer.
#[test]
#[ignore = "requires a running server"]
fn screen_stress() {
    const FPS: u32 = 60;
    const FRAME: usize = 22_000;
    const KEYFRAME: usize = 300_000;
    use shared::media::{SCREEN_FRAGMENT_DATA, ScreenFragment};
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let a = client(&rt);
    let b = client(&rt);
    a.connect("streamer", "secret", None);
    let (alice, _) = a.welcome();
    b.connect("watcher", "secret", None);
    b.welcome();
    a.send(ClientMsg::StartStream { kind: MediaKind::Screen });
    b.wait("live", |e| {
        matches!(e, NetEvent::Message(ServerMsg::UserUpdated(u)) if u.id == alice && !u.streams.is_empty())
            .then_some(())
    });
    let (tx, rx) = std::sync::mpsc::channel();
    b.screen.attach(alice, tx);
    b.send(ClientMsg::WatchStream { user_id: Some(alice) });
    std::thread::sleep(Duration::from_millis(300));

    let frames = FPS * 5;
    let mut sent = 0usize;
    let mut sent_key = 0usize;
    for id in 0..frames {
        let key = id % FPS == 0;
        let data = vec![id as u8; if key { KEYFRAME } else { FRAME }];
        let count = data.len().div_ceil(SCREEN_FRAGMENT_DATA);
        for (i, chunk) in data.chunks(SCREEN_FRAGMENT_DATA).enumerate() {
            let f = ScreenFragment { frame_id: id, index: i as u16, count: count as u16, keyframe: key, data: chunk };
            a.screen_tx.blocking_send((MediaKind::Screen, f.encode())).unwrap();
        }
        sent += count;
        if key {
            sent_key += count;
        }
        std::thread::sleep(Duration::from_secs(1) / FPS);
    }
    std::thread::sleep(Duration::from_millis(500));
    let mut got = std::collections::HashMap::<u32, usize>::new();
    while let Ok(p) = rx.try_recv() {
        let f = ScreenFragment::decode(&p).unwrap();
        *got.entry(f.frame_id).or_default() += 1;
    }
    let received: usize = got.values().sum();
    let complete = (0..frames)
        .filter(|id| {
            let need = if id % FPS == 0 { KEYFRAME } else { FRAME }.div_ceil(SCREEN_FRAGMENT_DATA);
            got.get(id).copied() == Some(need)
        })
        .count();
    let complete_key = (0..frames)
        .step_by(FPS as usize)
        .filter(|id| got.get(id).copied() == Some(KEYFRAME.div_ceil(SCREEN_FRAGMENT_DATA)))
        .count();
    println!(
        "fragments sent {sent} (keyframe {sent_key}), received {received} ({:.1}%); complete frames {complete}/{frames}, keyframes {complete_key}/{}",
        received as f64 * 100.0 / sent as f64,
        frames.div_ceil(FPS)
    );
}

/// Starts the server from CHUMLY_SERVER_BIN with config
/// CHUMLY_SERVER_CONFIG, kills it and starts it again.
#[test]
#[ignore = "requires the server binary path"]
fn reconnects_after_server_restart() {
    let bin = std::env::var("CHUMLY_SERVER_BIN").expect("CHUMLY_SERVER_BIN");
    let config = std::env::var("CHUMLY_SERVER_CONFIG").expect("CHUMLY_SERVER_CONFIG");
    let start = || {
        std::process::Command::new(&bin)
            .args(["--config", &config])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let mut server = start();
    std::thread::sleep(Duration::from_millis(800));

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let a = client(&rt);
    a.connect("dave", "secret", None);
    a.welcome();

    // A killed process sends no connection close, so the drop is
    // only noticed via the QUIC idle timeout (15 s).
    server.kill().unwrap();
    server.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match a.events.recv_timeout(left) {
            Ok(NetEvent::Reconnecting { .. }) => break,
            Ok(_) => {}
            Err(_) => panic!("client did not notice the server crash"),
        }
    }

    let mut server = start();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match a.events.recv_timeout(left) {
            Ok(NetEvent::Message(ServerMsg::Welcome { .. })) => break,
            Ok(NetEvent::Disconnected(r)) => panic!("client gave up: {r:?}"),
            Ok(_) => {}
            Err(_) => panic!("did not reconnect within 30 s"),
        }
    }
    a.cmds.send(NetCommand::Disconnect).unwrap();
    a.wait("disconnect", |e| matches!(e, NetEvent::Disconnected(DisconnectReason::User)).then_some(()));
    server.kill().unwrap();
    server.wait().unwrap();
}
