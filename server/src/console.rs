//! Admin commands in the server console.

use std::io::BufRead;
use std::sync::Arc;

use shared::ErrorCode;
use tokio::sync::mpsc::UnboundedSender;

use crate::bans::{fmt_remaining, parse_duration};
use crate::state::{Punish, Server};

const HELP: &str = "\
Commands:
  help                         this help
  list                         who is connected
  channels                     list channels with ids
  addchan <name>               create a channel
  renamechan <channel> <name>  rename a channel
  limit <channel> <n>          channel user limit; 0 = no limit
  delchan <channel>            delete a channel, move its users to the first free one
  kick <nick> [reason]         kick a user
  ban <nick> [time] [reason]   ban by nickname, IP and key; time: 30m, 12h,
                               7d, 2w (at most a year); without it, permanent
  unban <nick|ip|key>          remove a ban; for an ip also lifts the block
                               after too many wrong passwords; a key can be
                               given by its first 8 bytes as shown in list/bans
  bans                         list bans
  users                        registered users and their roles
  roles                        roles, most senior first, with their rights
  register <nick>              register an online user's nickname to their key
  unregister <nick>            remove a registration (and the role)
  grant <nick> <role>          give an online user a role by name (registers the
                               nickname); roles are edited in the client
  revoke <nick>                take the role away, the nickname stays registered
  owner <nick>                 make an online user the server owner; the previous
                               owner gets the next role down. The only way to move it
  rebind <registered> <online> bind a registration to the key of an online user,
                               e.g. after the server certificate was replaced
  setupcode                    show the setup code, if the server has no owner yet
  password                     whether the server has a password
  password set <password>      set or change it (asks to type it again)
  password remove              remove it: anyone with the address can join (asks
                               to confirm). Saved to server.toml; users already
                               connected stay
  say <text>                   announcement to all users
  quit                         stop the server

<channel> is an id from `channels` or a name without spaces.
Channel changes are saved to server.toml immediately.";

/// A command waiting for the next line to confirm it.
#[derive(Debug, PartialEq)]
enum Pending {
    /// The new password, to be typed again.
    SetPassword(String),
    /// "yes" removes the password.
    RemovePassword,
}

pub fn spawn(server: Arc<Server>, shutdown: UnboundedSender<()>) {
    std::thread::Builder::new()
        .name("console".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut pending = None;
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if exec(&server, line.trim(), &mut pending) {
                    let _ = shutdown.send(());
                    break;
                }
            }
        })
        .expect("spawn console thread");
}

/// Executes a command, or the confirmation of the pending one. Returns
/// `true` if the server should stop.
fn exec(server: &Server, line: &str, pending: &mut Option<Pending>) -> bool {
    if let Some(p) = pending.take() {
        let mut st = server.lock();
        match p {
            Pending::SetPassword(pw) if line == pw => {
                report(st.set_password(&pw, "console").map(|()| "Server password set.".to_string()));
            }
            Pending::RemovePassword if line.eq_ignore_ascii_case("yes") => {
                report(st.set_password("", "console").map(|()| "Server password removed.".to_string()));
            }
            Pending::SetPassword(_) => println!("The passwords do not match. Nothing changed."),
            Pending::RemovePassword => println!("Cancelled. The password stays."),
        }
        return false;
    }
    if line.is_empty() {
        return false;
    }
    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    let mut st = server.lock();
    match cmd.to_lowercase().as_str() {
        "help" | "?" => println!("{HELP}"),
        "list" | "who" => {
            if st.users.is_empty() {
                println!("Nobody is connected.");
            }
            let mut users: Vec<_> = st.users.values().collect();
            users.sort_by_key(|u| u.info.channel_id);
            for u in users {
                let i = &u.info;
                println!(
                    "  [{}] {:<20} {:<21} key {:<23} client {:<8} channel '{}'{}{}{}",
                    i.id,
                    i.nickname,
                    u.addr,
                    u.key.as_deref().map_or("-", crate::tls::short_key),
                    if u.client_version.is_empty() { "?" } else { &u.client_version },
                    st.channel_name(i.channel_id),
                    match i.role {
                        Some(_) => format!(" {}", st.role_name(i.role)),
                        None if i.registered => " registered".into(),
                        None => String::new(),
                    },
                    if i.muted { " muted" } else { "" },
                    if i.deafened { " deaf" } else { "" },
                );
            }
        }
        "channels" => {
            for c in &st.channels {
                let n = st.users.values().filter(|u| u.info.channel_id == c.id).count();
                let max = c.max_users.map(|m| format!("/{m}")).unwrap_or_default();
                println!("  [{}] {} ({n}{max})", c.id, c.name);
            }
        }
        "addchan" => {
            if rest.is_empty() {
                println!("Usage: addchan <name>");
            } else {
                report(
                    st.create_channel(rest, "", None, "console")
                        .map(|c| format!("Created channel [{}] {}", c.id, c.name)),
                );
            }
        }
        "renamechan" | "limit" | "delchan" => {
            let (query, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            let arg = arg.trim();
            let usage = match cmd.to_lowercase().as_str() {
                "renamechan" => "renamechan <channel> <new name>",
                "limit" => "limit <channel> <n, 0 = no limit>",
                _ => "delchan <channel>",
            };
            if query.is_empty() {
                println!("Usage: {usage}");
                return false;
            }
            let Some(id) = st.find_channel(query) else {
                println!("Channel '{query}' not found. See: channels");
                return false;
            };
            let Some(ch) = st.channels.iter().find(|c| c.id == id).cloned() else { return false };
            match cmd.to_lowercase().as_str() {
                "renamechan" if !arg.is_empty() => {
                    let r = st.edit_channel(id, arg, &ch.description, ch.max_users, "console");
                    report(r.map(|c| format!("Channel renamed: {}", c.name)));
                }
                "limit" => match arg.parse::<u32>() {
                    Ok(n) => {
                        let r = st.edit_channel(id, &ch.name, &ch.description, Some(n), "console");
                        report(r.map(|c| match c.max_users {
                            Some(m) => format!("Channel '{}' limit: {m}", c.name),
                            None => format!("Channel '{}' no longer has a limit", c.name),
                        }));
                    }
                    Err(_) => println!("Usage: {usage}"),
                },
                "delchan" => report(st.delete_channel(id, "console").map(|c| format!("Channel '{}' deleted", c.name))),
                _ => println!("Usage: {usage}"),
            }
        }
        "kick" | "ban" => {
            let ban = cmd.eq_ignore_ascii_case("ban");
            let (nick, mut reason) = rest.split_once(' ').unwrap_or((rest, ""));
            if nick.is_empty() {
                println!("Usage: {}", if ban { "ban <nick> [time] [reason]" } else { "kick <nick> [reason]" });
                return false;
            }
            let mut secs = None;
            if ban {
                let (first, after) = reason.trim().split_once(' ').unwrap_or((reason.trim(), ""));
                if let Some(s) = parse_duration(first) {
                    (secs, reason) = (Some(s), after);
                }
            }
            let punish = if ban { Punish::Ban { secs } } else { Punish::Kick };
            match st.find_by_nick(nick) {
                Some(id) => {
                    st.kick(id, reason.trim().to_string(), punish, None);
                }
                None => println!("User '{nick}' not found."),
            }
        }
        "unban" => {
            if rest.is_empty() {
                println!("Usage: unban <nick|ip|key>");
            } else {
                let n = st.bans.remove(rest);
                println!("Bans removed: {n}");
                // An IP is also lifted from the wrong-password block.
                if let Ok(ip) = rest.parse::<std::net::IpAddr>()
                    && st.auth.clear(ip)
                {
                    println!("Password attempt block lifted for {ip}");
                }
            }
        }
        "bans" => {
            let now = shared::unix_now();
            let bans = st.bans.active_at(now);
            if bans.is_empty() {
                println!("No bans.");
            }
            for b in bans {
                println!(
                    "  nick: {:<20} ip: {:<16} key: {:<23} ends: {:<10} by: {:<16} reason: {}",
                    b.nickname.as_deref().unwrap_or("-"),
                    b.ip.map(|i| i.to_string()).unwrap_or_else(|| "-".into()),
                    b.key.as_deref().map_or("-", crate::tls::short_key),
                    b.remaining_at(now).map_or("never".into(), |s| format!("in {}", fmt_remaining(s))),
                    b.by.as_deref().unwrap_or("console"),
                    if b.reason.is_empty() { "-" } else { &b.reason },
                );
            }
        }
        "users" => {
            if st.registry.users.is_empty() {
                println!("No registered users.");
            }
            for r in &st.registry.users {
                println!(
                    "  {:<20} key: {:<23} role: {}",
                    r.nickname,
                    crate::tls::short_key(&r.key),
                    st.role_name(r.role)
                );
            }
        }
        "roles" => {
            for r in &st.registry.roles.list {
                let n = st.registry.users.iter().filter(|u| u.role == Some(r.id)).count();
                let perms = st.registry.roles.perms(Some(r.id)).names().join(", ");
                println!("  {:<24} users: {n:<4} rights: {}", r.name, if perms.is_empty() { "-" } else { &perms });
            }
        }
        "register" | "grant" => {
            let (nick, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            let role = match (cmd.to_lowercase().as_str(), arg.trim()) {
                ("register", "") => None,
                ("grant", name) if !name.is_empty() => match st.registry.role_by_name(name) {
                    Some(r) => Some(Some(r.id)),
                    None => {
                        println!("No role '{name}'. Roles: see `roles`.");
                        return false;
                    }
                },
                _ => {
                    println!("Usage: register <nick> | grant <nick> <role>");
                    return false;
                }
            };
            let Some(id) = st.find_by_nick(nick) else {
                println!("User '{nick}' is not online.");
                return false;
            };
            let r = match role {
                None => st.set_registered(id, true, crate::state::Actor::CONSOLE, "console"),
                Some(role) => st.set_role(id, role, crate::state::Actor::CONSOLE, "console"),
            };
            match r {
                Ok(()) => println!("Done."),
                Err(code) => println!("{}", user_error(code)),
            }
        }
        "unregister" | "revoke" => {
            if rest.is_empty() {
                println!("Usage: {cmd} <nick>");
                return false;
            }
            let r = if cmd.eq_ignore_ascii_case("revoke") { st.revoke_nick(rest) } else { st.unregister_nick(rest) };
            match r {
                Ok(text) | Err(text) => println!("{text}"),
            }
        }
        "owner" => {
            if rest.is_empty() {
                println!("Usage: owner <nick>");
                return false;
            }
            match st.transfer_owner(rest) {
                Ok(text) | Err(text) => println!("{text}"),
            }
        }
        "rebind" => {
            let (registered, online) = rest.split_once(' ').unwrap_or((rest, ""));
            if registered.is_empty() || online.trim().is_empty() {
                println!("Usage: rebind <registered nick> <online nick>");
                return false;
            }
            match st.rebind(registered, online.trim()) {
                Ok(text) | Err(text) => println!("{text}"),
            }
        }
        "password" => {
            let (sub, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            match sub.to_lowercase().as_str() {
                "" => println!(
                    "Server password: {}",
                    if st.config.password.is_empty() { "none, anyone with the address can join" } else { "set" }
                ),
                "set" => {
                    let pw = arg.trim();
                    if pw.is_empty() {
                        println!("Usage: password set <password>  (to remove it: password remove)");
                    } else if !shared::validate_password(pw) {
                        println!(
                            "The password must be up to {} characters, without control characters.",
                            shared::MAX_PASSWORD_CHARS
                        );
                    } else {
                        println!("Type the new password again to confirm:");
                        *pending = Some(Pending::SetPassword(pw.to_string()));
                    }
                }
                "remove" => {
                    if st.config.password.is_empty() {
                        println!("The server has no password.");
                    } else {
                        println!(
                            "Remove the server password? Anyone who knows the address will be able to join. Type yes to confirm:"
                        );
                        *pending = Some(Pending::RemovePassword);
                    }
                }
                _ => println!("Usage: password | password set <password> | password remove"),
            }
        }
        "setupcode" => match &st.setup_code {
            Some(code) => println!("Admin setup code: {code}"),
            None => println!("The server has an owner already; no setup code."),
        },
        "say" => {
            if !rest.is_empty() {
                st.notice(rest);
                println!("Sent.");
            }
        }
        "quit" | "exit" | "stop" => return true,
        _ => println!("Unknown command '{cmd}'. Type help."),
    }
    false
}

fn report(r: Result<String, ErrorCode>) {
    match r {
        Ok(text) => println!("{text}"),
        Err(code) => println!(
            "{}",
            match code {
                ErrorCode::InvalidChannelName => "Invalid name: 1 to 32 characters, no control characters",
                ErrorCode::ChannelNameTaken => "A channel with this name already exists",
                ErrorCode::LastChannel => "Cannot delete the last channel",
                ErrorCode::NoSuchChannel => "Channel not found",
                ErrorCode::InvalidPassword => "Invalid password: up to 64 characters, no control characters",
                _ => "Failed (see the log)",
            }
        ),
    }
}

fn user_error(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::IdentityRequired => "This user has no key: roles and registration need one.",
        ErrorCode::NotAdmin => "Not allowed: the owner can only be changed with `owner <nick>`.",
        ErrorCode::CannotTargetSelf => "Not allowed on yourself.",
        ErrorCode::NoSuchUser => "User not found.",
        _ => "Failed.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bans::BanList;
    use crate::config::ServerConfig;
    use crate::users::Registry;

    fn server() -> Server {
        Server::new(ServerConfig::default(), BanList::default(), Registry::default())
    }

    #[test]
    fn password_needs_confirmation() {
        let s = server();
        let mut pending = None;
        exec(&s, "password set secret one", &mut pending);
        assert_eq!(pending, Some(Pending::SetPassword("secret one".into())));
        assert_eq!(s.lock().config.password, "", "not before the confirmation");
        exec(&s, "secret two", &mut pending);
        assert_eq!(s.lock().config.password, "", "mismatch changes nothing");
        assert_eq!(pending, None);

        exec(&s, "password set secret one", &mut pending);
        exec(&s, "secret one", &mut pending);
        assert_eq!(s.lock().config.password, "secret one");

        exec(&s, "password remove", &mut pending);
        exec(&s, "no", &mut pending);
        assert_eq!(s.lock().config.password, "secret one", "only yes removes it");
        exec(&s, "password remove", &mut pending);
        exec(&s, "YES", &mut pending);
        assert_eq!(s.lock().config.password, "");
        // Nothing to remove: no confirmation asked.
        exec(&s, "password remove", &mut pending);
        assert_eq!(pending, None);
    }
}
