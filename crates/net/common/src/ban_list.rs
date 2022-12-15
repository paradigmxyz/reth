//! Support for banning peers.
use reth_primitives::PeerId;
use std::{collections::HashMap, net::IpAddr, time::Instant};

/// Stores peers that should be taken out of circulation either indefinitely or until a certain
/// timestamp
#[derive(Debug, Clone, Default)]
pub struct BanList {
    /// A set of IPs whose packets get dropped instantly.
    banned_ips: HashMap<IpAddr, Option<Instant>>,
    /// A set of [`PeerId`] whose packets get dropped instantly.
    banned_peers: HashMap<PeerId, Option<Instant>>,
}

impl BanList {
    /// Creates a new ban list that bans the given peers and ips indefinitely.
    pub fn new(
        banned_peers: impl IntoIterator<Item = PeerId>,
        banned_ips: impl IntoIterator<Item = IpAddr>,
    ) -> Self {
        Self {
            banned_ips: banned_ips.into_iter().map(|ip| (ip, None)).collect(),
            banned_peers: banned_peers.into_iter().map(|peer| (peer, None)).collect(),
        }
    }

    /// Creates a new ban list that bans the given peers and ips with an optional timeout.
    pub fn new_with_timeout(
        banned_peers: HashMap<PeerId, Option<Instant>>,
        banned_ips: HashMap<IpAddr, Option<Instant>>,
    ) -> Self {
        Self { banned_ips, banned_peers }
    }

    /// Removes all entries that should no longer be banned.
    pub fn evict(&mut self, now: Instant) {
        self.banned_ips.retain(|_, until| {
            if let Some(until) = until {
                return *until > now
            }
            true
        });

        self.banned_peers.retain(|_, until| {
            if let Some(until) = until {
                return *until > now
            }
            true
        });
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
    pub fn ban_ip_until(&mut self, ip: IpAddr, until: Instant) {
        self.ban_ip_with(ip, Some(until));
    }

    /// Bans the peer until the timestamp
    pub fn ban_peer_until(&mut self, node_id: PeerId, until: Instant) {
        self.ban_peer_with(node_id, Some(until));
    }

    /// Bans the IP indefinitely.
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
    pub fn ban_ip_with(&mut self, ip: IpAddr, until: Option<Instant>) {
        self.banned_ips.insert(ip, until);
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
        let ip = IpAddr::from([0, 0, 0, 0]);
        let mut banlist = BanList::default();
        banlist.ban_ip(ip);
        assert!(banlist.is_banned_ip(&ip));
        banlist.unban_ip(&ip);
        assert!(!banlist.is_banned_ip(&ip));
    }
}
