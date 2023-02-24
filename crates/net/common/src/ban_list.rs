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
    banned_peers: HashMap<PeerId, Option<Instant>>,
}

impl BanList {
    /// Creates a new ban list that bans the given peers and ips indefinitely.
    ///
    /// This will ban non-global IPs if they are included in `banned_ips`
    pub fn new(
        banned_peers: impl IntoIterator<Item = PeerId>,
        banned_ips: impl IntoIterator<Item = IpAddr>,
    ) -> Self {
        Self::new_with_timeout(
            banned_peers.into_iter().map(|peer| (peer, None)).collect(),
            banned_ips.into_iter().map(|ip| (ip, None)).collect(),
        )
    }

    /// Creates a new ban list that bans the given peers and ips with an optional timeout.
    ///
    /// This will ban non-global IPs if they are included in `banned_ips`
    pub fn new_with_timeout(
        banned_peers: HashMap<PeerId, Option<Instant>>,
        banned_ips: HashMap<IpAddr, Option<Instant>>,
    ) -> Self {
        Self { banned_ips, banned_peers }
    }

    /// Removes all peers that are no longer banned.
    pub fn evict_peers(&mut self, now: Instant) -> Vec<PeerId> {
        let mut evicted = Vec::new();
        self.banned_peers.retain(|peer, until| {
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

    /// Removes all ip addresses that are no longer banned.
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
        let peers = self.evict_peers(now);
        (ips, peers)
    }

    /// Returns true if either the given peer id _or_ ip address is banned.
    #[inline]
    pub fn is_banned(&self, peer_id: &PeerId, ip: &IpAddr) -> bool {
        self.is_banned_peer(peer_id) || self.is_banned_ip(ip)
    }

    /// checks the ban list to see if it contains the given ip
    #[inline]
    pub fn is_banned_ip(&self, ip: &IpAddr) -> bool {
        self.banned_ips.contains_key(ip)
    }

    /// checks the ban list to see if it contains the given ip
    #[inline]
    pub fn is_banned_peer(&self, peer_id: &PeerId) -> bool {
        self.banned_peers.contains_key(peer_id)
    }

    /// Unbans the ip address
    pub fn unban_ip(&mut self, ip: &IpAddr) {
        self.banned_ips.remove(ip);
    }

    /// Unbans the ip address
    pub fn unban_peer(&mut self, peer_id: &PeerId) {
        self.banned_peers.remove(peer_id);
    }

    /// Bans the IP until the timestamp.
    ///
    /// This does not ban non-global IPs.
    pub fn ban_ip_until(&mut self, ip: IpAddr, until: Instant) {
        self.ban_ip_with(ip, Some(until));
    }

    /// Bans the peer until the timestamp
    pub fn ban_peer_until(&mut self, node_id: PeerId, until: Instant) {
        self.ban_peer_with(node_id, Some(until));
    }

    /// Bans the IP indefinitely.
    ///
    /// This does not ban non-global IPs.
    pub fn ban_ip(&mut self, ip: IpAddr) {
        self.ban_ip_with(ip, None);
    }

    /// Bans the peer indefinitely,
    pub fn ban_peer(&mut self, node_id: PeerId) {
        self.ban_peer_with(node_id, None);
    }

    /// Bans the peer indefinitely or until the given timeout.
    pub fn ban_peer_with(&mut self, node_id: PeerId, until: Option<Instant>) {
        self.banned_peers.insert(node_id, until);
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
    fn can_ban_unban_peer() {
        let peer = PeerId::random();
        let mut banlist = BanList::default();
        banlist.ban_peer(peer);
        assert!(banlist.is_banned_peer(&peer));
        banlist.unban_peer(&peer);
        assert!(!banlist.is_banned_peer(&peer));
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
