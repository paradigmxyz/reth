//! Support for banning peers.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

type PeerId = alloy_primitives::B512;

use std::{collections::HashMap, net::IpAddr, str::FromStr, time::Instant};

/// Determines whether or not the IP is globally routable.
/// Should be replaced with [`IpAddr::is_global`](std::net::IpAddr::is_global) once it is stable.
pub const fn is_global(ip: &IpAddr) -> bool {
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
        Self::new_with_timeout(
            banned_peers.into_iter().map(|peer| (peer, None)).collect(),
            banned_ips.into_iter().map(|ip| (ip, None)).collect(),
        )
    }

    /// Creates a new ban list that bans the given peers and ips with an optional timeout.
    pub const fn new_with_timeout(
        banned_peers: HashMap<PeerId, Option<Instant>>,
        banned_ips: HashMap<IpAddr, Option<Instant>>,
    ) -> Self {
        Self { banned_ips, banned_peers }
    }

    /// Removes all peers that are no longer banned.
    pub fn evict_peers(&mut self, now: Instant) -> Vec<PeerId> {
        let mut evicted = Vec::new();
        self.banned_peers.retain(|peer, until| {
            if let Some(until) = until &&
                now > *until
            {
                evicted.push(*peer);
                return false
            }
            true
        });
        evicted
    }

    /// Removes all ip addresses that are no longer banned.
    pub fn evict_ips(&mut self, now: Instant) -> Vec<IpAddr> {
        let mut evicted = Vec::new();
        self.banned_ips.retain(|peer, until| {
            if let Some(until) = until &&
                now > *until
            {
                evicted.push(*peer);
                return false
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

    /// checks the ban list to see if it contains the given peer
    #[inline]
    pub fn is_banned_peer(&self, peer_id: &PeerId) -> bool {
        self.banned_peers.contains_key(peer_id)
    }

    /// Unbans the ip address
    pub fn unban_ip(&mut self, ip: &IpAddr) {
        self.banned_ips.remove(ip);
    }

    /// Unbans the peer
    pub fn unban_peer(&mut self, peer_id: &PeerId) {
        self.banned_peers.remove(peer_id);
    }

    /// Bans the IP until the timestamp.
    ///
    /// This does not ban non-global IPs.
    /// If the IP is already banned, the timeout will be updated to the new value.
    pub fn ban_ip_until(&mut self, ip: IpAddr, until: Instant) {
        self.ban_ip_with(ip, Some(until));
    }

    /// Bans the peer until the timestamp.
    ///
    /// If the peer is already banned, the timeout will be updated to the new value.
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
    ///
    /// If the peer is already banned, the timeout will be updated to the new value.
    pub fn ban_peer_with(&mut self, node_id: PeerId, until: Option<Instant>) {
        self.banned_peers.insert(node_id, until);
    }

    /// Bans the ip indefinitely or until the given timeout.
    ///
    /// This does not ban non-global IPs.
    /// If the IP is already banned, the timeout will be updated to the new value.
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
        let peer = PeerId::new([1; 64]);
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

/// IP filter for restricting network communication to specific IP ranges using CIDR notation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpFilter {
    /// List of allowed IP networks in CIDR notation.
    /// If empty, all IPs are allowed.
    allowed_networks: Vec<ipnet::IpNet>,
}

impl IpFilter {
    /// Creates a new IP filter with the given CIDR networks.
    ///
    /// If the list is empty, all IPs will be allowed.
    pub const fn new(allowed_networks: Vec<ipnet::IpNet>) -> Self {
        Self { allowed_networks }
    }

    /// Creates an IP filter from a comma-separated list of CIDR networks.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the CIDR strings cannot be parsed.
    pub fn from_cidr_string(cidrs: &str) -> Result<Self, ipnet::AddrParseError> {
        if cidrs.is_empty() {
            return Ok(Self::allow_all())
        }

        let networks = cidrs
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ipnet::IpNet::from_str)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self::new(networks))
    }

    /// Creates a filter that allows all IPs.
    pub const fn allow_all() -> Self {
        Self { allowed_networks: Vec::new() }
    }

    /// Checks if the given IP address is allowed by this filter.
    ///
    /// Returns `true` if the filter is empty (allows all) or if the IP is within
    /// any of the allowed networks.
    pub fn is_allowed(&self, ip: &IpAddr) -> bool {
        // If no restrictions are set, allow all IPs
        if self.allowed_networks.is_empty() {
            return true
        }

        // Check if the IP is within any of the allowed networks
        self.allowed_networks.iter().any(|net| net.contains(ip))
    }

    /// Returns `true` if this filter has restrictions (i.e., not allowing all IPs).
    pub const fn has_restrictions(&self) -> bool {
        !self.allowed_networks.is_empty()
    }

    /// Returns the list of allowed networks.
    pub fn allowed_networks(&self) -> &[ipnet::IpNet] {
        &self.allowed_networks
    }
}

impl Default for IpFilter {
    fn default() -> Self {
        Self::allow_all()
    }
}

#[cfg(test)]
mod ip_filter_tests {
    use super::*;

    #[test]
    fn test_allow_all_filter() {
        let filter = IpFilter::allow_all();
        assert!(filter.is_allowed(&IpAddr::from([192, 168, 1, 1])));
        assert!(filter.is_allowed(&IpAddr::from([10, 0, 0, 1])));
        assert!(filter.is_allowed(&IpAddr::from([8, 8, 8, 8])));
        assert!(!filter.has_restrictions());
    }

    #[test]
    fn test_single_network_filter() {
        let filter = IpFilter::from_cidr_string("192.168.0.0/16").unwrap();
        assert!(filter.is_allowed(&IpAddr::from([192, 168, 1, 1])));
        assert!(filter.is_allowed(&IpAddr::from([192, 168, 255, 255])));
        assert!(!filter.is_allowed(&IpAddr::from([192, 169, 1, 1])));
        assert!(!filter.is_allowed(&IpAddr::from([10, 0, 0, 1])));
        assert!(filter.has_restrictions());
    }

    #[test]
    fn test_multiple_networks_filter() {
        let filter = IpFilter::from_cidr_string("192.168.0.0/16,10.0.0.0/8").unwrap();
        assert!(filter.is_allowed(&IpAddr::from([192, 168, 1, 1])));
        assert!(filter.is_allowed(&IpAddr::from([10, 5, 10, 20])));
        assert!(filter.is_allowed(&IpAddr::from([10, 255, 255, 255])));
        assert!(!filter.is_allowed(&IpAddr::from([172, 16, 0, 1])));
        assert!(!filter.is_allowed(&IpAddr::from([8, 8, 8, 8])));
    }

    #[test]
    fn test_ipv6_filter() {
        let filter = IpFilter::from_cidr_string("2001:db8::/32").unwrap();
        let ipv6_in_range: IpAddr = "2001:db8::1".parse().unwrap();
        let ipv6_out_range: IpAddr = "2001:db9::1".parse().unwrap();

        assert!(filter.is_allowed(&ipv6_in_range));
        assert!(!filter.is_allowed(&ipv6_out_range));
    }

    #[test]
    fn test_mixed_ipv4_ipv6_filter() {
        let filter = IpFilter::from_cidr_string("192.168.0.0/16,2001:db8::/32").unwrap();

        assert!(filter.is_allowed(&IpAddr::from([192, 168, 1, 1])));
        let ipv6_in_range: IpAddr = "2001:db8::1".parse().unwrap();
        assert!(filter.is_allowed(&ipv6_in_range));

        assert!(!filter.is_allowed(&IpAddr::from([10, 0, 0, 1])));
        let ipv6_out_range: IpAddr = "2001:db9::1".parse().unwrap();
        assert!(!filter.is_allowed(&ipv6_out_range));
    }

    #[test]
    fn test_empty_string() {
        let filter = IpFilter::from_cidr_string("").unwrap();
        assert!(filter.is_allowed(&IpAddr::from([192, 168, 1, 1])));
        assert!(!filter.has_restrictions());
    }

    #[test]
    fn test_invalid_cidr() {
        assert!(IpFilter::from_cidr_string("invalid").is_err());
        assert!(IpFilter::from_cidr_string("192.168.0.0/33").is_err());
        assert!(IpFilter::from_cidr_string("192.168.0.0,10.0.0.0").is_err());
    }

    #[test]
    fn test_whitespace_handling() {
        let filter = IpFilter::from_cidr_string(" 192.168.0.0/16 , 10.0.0.0/8 ").unwrap();
        assert!(filter.is_allowed(&IpAddr::from([192, 168, 1, 1])));
        assert!(filter.is_allowed(&IpAddr::from([10, 0, 0, 1])));
        assert!(!filter.is_allowed(&IpAddr::from([172, 16, 0, 1])));
    }

    #[test]
    fn test_single_ip_as_cidr() {
        let filter = IpFilter::from_cidr_string("192.168.1.100/32").unwrap();
        assert!(filter.is_allowed(&IpAddr::from([192, 168, 1, 100])));
        assert!(!filter.is_allowed(&IpAddr::from([192, 168, 1, 101])));
    }
}
