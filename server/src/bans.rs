use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::limits::ip_key;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ban {
    /// Nickname at ban time. Compared case-insensitively.
    pub nickname: Option<String>,
    /// Address at ban time. An IPv6 ban covers the whole /64: one
    /// connection usually gets a /64 and can pick any address in it.
    pub ip: Option<IpAddr>,
    /// The user's public key fingerprint ("AB:CD:..."), if they had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub reason: String,
    pub created: i64,
    /// Unix time the ban ends; absent: permanent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,
    /// Nickname of the moderator who issued it; absent: the server console.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

impl Ban {
    fn info(&self) -> shared::BanInfo {
        shared::BanInfo {
            nickname: self.nickname.clone(),
            ip: self.ip.map(|i| i.to_string()),
            key: self.key.clone(),
            reason: self.reason.clone(),
            created: self.created,
            expires: self.expires,
            by: self.by.clone(),
        }
    }

    fn active_at(&self, now: i64) -> bool {
        self.expires.is_none_or(|e| e > now)
    }

    /// Seconds until the ban ends at `now`; `None`: permanent.
    pub fn remaining_at(&self, now: i64) -> Option<u64> {
        self.expires.map(|e| u64::try_from(e - now).unwrap_or(0))
    }
}

/// The end of a ban of `duration_secs` starting at `now`: `None` (permanent)
/// for no duration; at most `shared::MAX_BAN_SECS`, at least a minute.
pub fn expiry(now: i64, duration_secs: Option<u64>) -> Option<i64> {
    let secs = duration_secs?.clamp(60, shared::MAX_BAN_SECS);
    Some(now + secs as i64)
}

/// A console duration: a number and a unit, `30m`, `12h`, `7d`, `2w`.
pub fn parse_duration(s: &str) -> Option<u64> {
    let unit = match s.chars().last()? {
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        'w' => 7 * 86_400,
        _ => return None,
    };
    let n: u64 = s[..s.len() - 1].parse().ok().filter(|n| *n > 0)?;
    n.checked_mul(unit).filter(|s| *s <= shared::MAX_BAN_SECS)
}

/// Time left for the console and the log: "3d 4h", "2h 5m", "40m", "<1m".
pub fn fmt_remaining(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h, m) {
        (0, 0, 0) => "<1m".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BanList {
    #[serde(default)]
    pub bans: Vec<Ban>,
    #[serde(skip)]
    path: PathBuf,
}

impl BanList {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut list: BanList =
            if path.exists() { toml::from_str(&std::fs::read_to_string(path)?)? } else { BanList::default() };
        list.path = path.to_path_buf();
        list.prune_at(shared::unix_now());
        Ok(list)
    }

    /// Drops bans that have ended, saving the list if any did. Expired bans
    /// never match anyway (`find`); this only keeps the file and the lists
    /// tidy.
    pub fn prune_at(&mut self, now: i64) {
        let before = self.bans.len();
        self.bans.retain(|b| b.active_at(now));
        if self.bans.len() != before {
            self.save();
        }
    }

    /// Bans in force, ending soonest first, permanent last.
    pub fn active_at(&self, now: i64) -> Vec<&Ban> {
        let mut list: Vec<&Ban> = self.bans.iter().filter(|b| b.active_at(now)).collect();
        list.sort_by_key(|b| (b.expires.is_none(), b.expires, std::cmp::Reverse(b.created)));
        list
    }

    pub fn save(&self) {
        let res = toml::to_string_pretty(self).map_err(anyhow::Error::from).and_then(|s| {
            // Through a temp file, so a failed write keeps the old list.
            let tmp = self.path.with_extension("toml.tmp");
            std::fs::write(&tmp, s)?;
            std::fs::rename(&tmp, &self.path)?;
            Ok(())
        });
        if let Err(e) = res {
            tracing::error!("failed to save ban list: {e:#}");
        }
    }

    #[cfg(test)]
    fn find(&self, nickname: &str, ip: IpAddr, key: Option<&str>) -> Option<&Ban> {
        self.find_at(nickname, ip, key, shared::unix_now())
    }

    /// The ban in force at `now` that covers this nickname, address or key.
    /// If several do, the one that ends last (permanent first).
    pub fn find_at(&self, nickname: &str, ip: IpAddr, key: Option<&str>, now: i64) -> Option<&Ban> {
        let nick = nickname.to_lowercase();
        let ip = ip_key(ip);
        self.bans
            .iter()
            .filter(|b| b.active_at(now))
            .filter(|b| {
                b.ip.map(ip_key) == Some(ip)
                    || b.nickname.as_deref().is_some_and(|n| n.to_lowercase() == nick)
                    || (b.key.is_some() && b.key.as_deref() == key)
            })
            .max_by_key(|b| b.expires.unwrap_or(i64::MAX))
    }

    /// For the admin panel: bans in force, ending soonest first.
    pub fn infos(&self) -> Vec<shared::BanInfo> {
        self.active_at(shared::unix_now()).into_iter().map(Ban::info).collect()
    }

    /// Removes the ban the admin panel showed as `info`. Returns false if
    /// there is no such ban (already lifted).
    pub fn remove_exact(&mut self, info: &shared::BanInfo) -> bool {
        let Some(i) = self.bans.iter().position(|b| b.info() == *info) else { return false };
        self.bans.remove(i);
        self.save();
        true
    }

    pub fn add(&mut self, ban: Ban) {
        let now = shared::unix_now();
        self.bans.retain(|b| b.active_at(now));
        self.bans.push(ban);
        self.save();
    }

    /// Removes bans matching the nickname, IP or key (the full fingerprint or
    /// at least its first 8 bytes, colons optional). Returns how many were removed.
    pub fn remove(&mut self, query: &str) -> usize {
        let key: Option<IpAddr> = query.parse().ok().map(ip_key);
        let q = query.to_lowercase();
        let before = self.bans.len();
        self.bans.retain(|b| {
            let by_nick = b.nickname.as_deref().is_some_and(|n| n.to_lowercase() == q);
            let by_ip = key.is_some() && b.ip.map(ip_key) == key;
            let by_key = b.key.as_deref().is_some_and(|k| key_matches(k, query));
            !(by_nick || by_ip || by_key)
        });
        let removed = before - self.bans.len();
        if removed > 0 {
            self.save();
        }
        removed
    }
}

/// `query` names the key `key`: its full fingerprint or a prefix of at
/// least 8 bytes, in any case, with or without colons.
fn key_matches(key: &str, query: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| *c != ':').collect::<String>().to_uppercase();
    let (key, query) = (norm(key), norm(query));
    query.len() >= 16 && query.chars().all(|c| c.is_ascii_hexdigit()) && key.starts_with(&query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ban(nick: &str, ip: &str) -> Ban {
        Ban {
            nickname: Some(nick.into()),
            ip: Some(ip.parse().unwrap()),
            key: None,
            reason: String::new(),
            created: 0,
            expires: None,
            by: None,
        }
    }

    #[test]
    fn ipv6_ban_covers_the_64() {
        let dir = tempfile::tempdir().unwrap();
        let mut list = BanList::load(&dir.path().join("bans.toml")).unwrap();
        list.add(ban("eve", "2001:db8:1:2::10"));
        list.add(ban("mallory", "10.0.0.1"));
        assert!(list.find("other", "2001:db8:1:2:ffff::1".parse().unwrap(), None).is_some());
        assert!(list.find("other", "2001:db8:1:3::10".parse().unwrap(), None).is_none());
        assert!(list.find("other", "10.0.0.1".parse().unwrap(), None).is_some());
        assert!(list.find("other", "10.0.0.2".parse().unwrap(), None).is_none());
        assert!(list.find("EVE", "10.9.9.9".parse().unwrap(), None).is_some());
        // Unban by any address of the /64.
        assert_eq!(list.remove("2001:db8:1:2::99"), 1);
        assert!(list.find("other", "2001:db8:1:2::10".parse().unwrap(), None).is_none());
        let reloaded = BanList::load(&dir.path().join("bans.toml")).unwrap();
        assert_eq!(reloaded.bans.len(), 1);
    }

    #[test]
    fn ban_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut list = BanList::load(&dir.path().join("bans.toml")).unwrap();
        let key = "AB:CD:EF:01:23:45:67:89:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11";
        list.add(Ban {
            nickname: Some("eve".into()),
            ip: Some("10.0.0.1".parse().unwrap()),
            key: Some(key.into()),
            reason: String::new(),
            created: 0,
            expires: None,
            by: None,
        });
        // New nickname, new address, same key: still banned.
        assert!(list.find("evelyn", "10.9.9.9".parse().unwrap(), Some(key)).is_some());
        assert!(list.find("evelyn", "10.9.9.9".parse().unwrap(), None).is_none());
        // A key prefix shorter than 8 bytes does not unban.
        assert_eq!(list.remove("AB:CD:EF"), 0);
        assert_eq!(list.remove("abcdef0123456789"), 1);
        assert!(list.bans.is_empty());
    }

    #[test]
    fn remove_exact_by_info() {
        let dir = tempfile::tempdir().unwrap();
        let mut list = BanList::load(&dir.path().join("bans.toml")).unwrap();
        list.add(ban("eve", "10.0.0.1"));
        list.add(ban("mallory", "10.0.0.2"));
        let infos = list.infos();
        assert_eq!(infos.len(), 2);
        assert!(list.remove_exact(&infos[0]));
        assert!(!list.remove_exact(&infos[0]), "already lifted");
        assert_eq!(list.bans.len(), 1);
        assert_eq!(list.bans[0].nickname.as_deref(), Some("mallory"));
    }

    #[test]
    fn temporary_ban_ends_by_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bans.toml");
        let mut list = BanList::load(&path).unwrap();
        let now = shared::unix_now();
        list.add(Ban { expires: Some(now + 3600), by: Some("mod".into()), ..ban("eve", "10.0.0.1") });
        let ip = "10.0.0.1".parse().unwrap();
        let b = list.find_at("x", ip, None, now).expect("in force");
        assert_eq!(b.remaining_at(now), Some(3600));
        assert!(list.find_at("x", ip, None, now + 3599).is_some());
        assert!(list.find_at("x", ip, None, now + 3600).is_none(), "ended");

        // Saved with its end and author, and dropped from the file once over.
        let reloaded = BanList::load(&path).unwrap();
        assert_eq!(reloaded.bans[0].expires, Some(now + 3600));
        assert_eq!(reloaded.bans[0].by.as_deref(), Some("mod"));
        let mut reloaded = reloaded;
        reloaded.prune_at(now + 3600);
        assert!(BanList::load(&path).unwrap().bans.is_empty());
    }

    #[test]
    fn the_longest_ban_wins_and_lists_end_soonest_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut list = BanList::load(&dir.path().join("bans.toml")).unwrap();
        let now = shared::unix_now();
        list.add(Ban { expires: Some(now + 60), ..ban("eve", "10.0.0.1") });
        list.add(ban("eve2", "10.0.0.1"));
        list.add(Ban { expires: Some(now + 30), ..ban("bob", "10.0.0.2") });
        let ip = "10.0.0.1".parse().unwrap();
        assert_eq!(list.find_at("x", ip, None, now).unwrap().expires, None, "permanent beats temporary");
        let order: Vec<_> = list.active_at(now).iter().map(|b| b.nickname.clone().unwrap()).collect();
        assert_eq!(order, ["bob", "eve", "eve2"]);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("1h"), Some(3600));
        assert_eq!(parse_duration("7d"), Some(7 * 86_400));
        assert_eq!(parse_duration("2w"), Some(14 * 86_400));
        for bad in ["", "h", "0h", "-1h", "1x", "1.5h", "spam", "9999w"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
        assert_eq!(expiry(100, None), None);
        assert_eq!(expiry(100, Some(5)), Some(160), "at least a minute");
        assert_eq!(expiry(0, Some(u64::MAX)), Some(shared::MAX_BAN_SECS as i64));
        assert_eq!(fmt_remaining(30), "<1m");
        assert_eq!(fmt_remaining(2400), "40m");
        assert_eq!(fmt_remaining(7500), "2h 5m");
        assert_eq!(fmt_remaining(3 * 86_400 + 4 * 3600 + 59), "3d 4h");
    }
}
