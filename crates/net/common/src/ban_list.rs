//! Support for banning peers.

use reth_primitives::PeerId;
use std::{collections::HashMap, net::IpAddr, time::Instant};

/// Determines whether or not the IP is globally routable.
/// Should be replaced with [`IpAddr::is_global`](std::net::IpAddr::is_global) once it is stable.
pub fn is_global(ip: &IpAddr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() {
        return false
    }

    match ip {
        IpAddr::V4(ip) => !ip.is_private() && !ip.is_link_local(),
        IpAddr::V6(_) => true,
    }
}

/// Stores peers that should be taken out of circulation either indefinitely or until a certain
/// timestamp
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BanList {
    /// A set of IPs whose packets get dropped instantly.
    banned_ips: HashMap<IpAddr, Option<Instant>>,
    /// A set of [`PeerId`] whose packets get dropped instantly.
    banned_ids: HashMap<PeerId, Option<Instant>>,
}

impl BanList {
    /// Creates a new ban list that bans the given peers and ips indefinitely.
    pub fn new(
        banned_ids: impl IntoIterator<Item = PeerId>,
        banned_ips: impl IntoIterator<Item = IpAddr>,
    ) -> Self {
        Self::new_with_timeout(
            banned_ids.into_iter().map(|id| (id, None)).collect(),
            banned_ips.into_iter().map(|ip| (ip, None)).collect(),
        )
    }

    /// Creates a new ban list that bans the given peer ids and ips with an optional timeout.
    pub fn new_with_timeout(
        banned_ids: HashMap<PeerId, Option<Instant>>,
        banned_ips: HashMap<IpAddr, Option<Instant>>,
    ) -> Self {
        Self { banned_ips, banned_ids }
    }

    /// Removes all peer ids that are no longer banned.
    pub fn evict_ids(&mut self, now: Instant) -> Vec<PeerId> {
        let mut evicted = Vec::new();
        self.banned_ids.retain(|peer, until| {
            if let Some(until) = until {
                if now > *until {
                    evicted.push(*peer);
                    return false
                }
            }
            true
        });
        evicted
    }

    /// Removes all peer ips that are no longer banned.
    pub fn evict_ips(&mut self, now: Instant) -> Vec<IpAddr> {
        let mut evicted = Vec::new();
        self.banned_ips.retain(|peer, until| {
            if let Some(until) = until {
                if now > *until {
                    evicted.push(*peer);
                    return false
                }
            }
            true
        });
        evicted
    }

    /// Removes all entries that should no longer be banned.
    ///
    /// Returns the evicted entries.
    pub fn evict(&mut self, now: Instant) -> (Vec<IpAddr>, Vec<PeerId>) {
        let ips = self.evict_ips(now);
        let ids = self.evict_ids(now);
        (ips, ids)
    }

    /// Returns true if either the given peer id _or_ ip address is banned.
    #[inline]
    pub fn is_banned(&self, id: &PeerId, ip: &IpAddr) -> bool {
        self.is_banned_id(id) || self.is_banned_ip(ip)
    }

    /// checks the ban list to see if it contains the given ip
    #[inline]
    pub fn is_banned_ip(&self, ip: &IpAddr) -> bool {
        self.banned_ips.contains_key(ip)
    }

    /// checks the ban list to see if it contains the given id
    #[inline]
    pub fn is_banned_id(&self, id: &PeerId) -> bool {
        self.banned_ids.contains_key(id)
    }

    /// Unbans the peer ip address
    pub fn unban_ip(&mut self, ip: &IpAddr) {
        self.banned_ips.remove(ip);
    }

    /// Unbans the peer id
    pub fn unban_id(&mut self, id: &PeerId) {
        self.banned_ids.remove(id);
    }

    /// Bans the IP until the timestamp.
    ///
    /// This does not ban non-global IPs.
    pub fn ban_ip_until(&mut self, ip: IpAddr, until: Instant) {
        self.ban_ip_with(ip, Some(until));
    }

    /// Bans the peer until the timestamp
    pub fn ban_id_until(&mut self, id: PeerId, until: Instant) {
        self.ban_id_with(id, Some(until));
    }

    /// Bans the IP indefinitely.
    ///
    /// This does not ban non-global IPs.
    pub fn ban_ip(&mut self, ip: IpAddr) {
        self.ban_ip_with(ip, None);
    }

    /// Bans the peer indefinitely,
    pub fn ban_id(&mut self, id: PeerId) {
        self.ban_id_with(id, None);
    }

    /// Bans the peer indefinitely or until the given timeout.
    pub fn ban_id_with(&mut self, id: PeerId, until: Option<Instant>) {
        self.banned_ids.insert(id, until);
    }

    /// Bans the ip indefinitely or until the given timeout.
    ///
    /// This does not ban non-global IPs.
    pub fn ban_ip_with(&mut self, ip: IpAddr, until: Option<Instant>) {
        if is_global(&ip) {
            self.banned_ips.insert(ip, until);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_ban_unban_id() {
        let id = PeerId::random();
        let mut banlist = BanList::default();
        banlist.ban_id(id);
        assert!(banlist.is_banned_id(&id));
        banlist.unban_id(&id);
        assert!(!banlist.is_banned_id(&id));
    }

    #[test]
    fn can_ban_unban_ip() {
        let ip = IpAddr::from([1, 1, 1, 1]);
        let mut banlist = BanList::default();
        banlist.ban_ip(ip);
        assert!(banlist.is_banned_ip(&ip));
        banlist.unban_ip(&ip);
        assert!(!banlist.is_banned_ip(&ip));
    }

    #[test]
    fn cannot_ban_non_global() {
        let mut ip = IpAddr::from([0, 0, 0, 0]);
        let mut banlist = BanList::default();
        banlist.ban_ip(ip);
        assert!(!banlist.is_banned_ip(&ip));

        ip = IpAddr::from([10, 0, 0, 0]);
        banlist.ban_ip(ip);
        assert!(!banlist.is_banned_ip(&ip));

        ip = IpAddr::from([127, 0, 0, 0]);
        banlist.ban_ip(ip);
        assert!(!banlist.is_banned_ip(&ip));

        ip = IpAddr::from([172, 17, 0, 0]);
        banlist.ban_ip(ip);
        assert!(!banlist.is_banned_ip(&ip));

        ip = IpAddr::from([172, 16, 0, 0]);
        banlist.ban_ip(ip);
        assert!(!banlist.is_banned_ip(&ip));
    }
}
