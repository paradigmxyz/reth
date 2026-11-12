//! Per-subnet limits for the discv4 routing table.

use crate::{NodeEntry, NodeRecord};
use std::net::IpAddr;

/// Maximum number of nodes from the same subnet in the whole table.
pub(crate) const MAX_NODES_PER_SUBNET_TABLE: usize = 10;

/// Maximum number of nodes from the same subnet in a single bucket.
pub(crate) const MAX_NODES_PER_SUBNET_BUCKET: usize = 2;

/// The network prefix that nodes are grouped by: IPv4 /24 or IPv6 /48.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subnet {
    V4([u8; 3]),
    V6([u8; 6]),
}

/// Returns whether another node fits alongside the proven entries in this subnet.
///
/// Unproven records cannot reserve quota for addresses supplied by third parties.
pub(crate) fn subnet_filter<'a>(
    record: &NodeRecord,
    other_vals: impl Iterator<Item = &'a NodeEntry>,
    limit: usize,
) -> bool {
    let Some(target) = subnet(record.address) else { return true };

    other_vals
        .filter(|other| other.has_endpoint_proof && other.record.id != record.id)
        .filter(|other| subnet(other.record.address) == Some(target))
        .take(limit)
        .count() <
        limit
}

/// Returns the subnet of the given address, or `None` if the address is exempt from limiting.
///
/// Records are stored IPv4-mapped, so the address is canonicalized first. Like geth's
/// `netutil.IsLAN`, loopback, unspecified, private, link-local and unique-local addresses are
/// exempt so that local networks and devnets are never limited.
const fn subnet(ip: IpAddr) -> Option<Subnet> {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            if ip.is_loopback() || ip.is_unspecified() || ip.is_private() || ip.is_link_local() {
                return None
            }
            let [a, b, c, _] = ip.octets();
            Some(Subnet::V4([a, b, c]))
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback() ||
                ip.is_unspecified() ||
                ip.is_unique_local() ||
                ip.is_unicast_link_local()
            {
                return None
            }
            let [a, b, c, d, e, f, ..] = ip.octets();
            Some(Subnet::V6([a, b, c, d, e, f]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_network_peers::{NodeRecord, PeerId};

    fn entry(ip: &str) -> NodeEntry {
        NodeEntry::new_proven(NodeRecord::new(
            (ip.parse::<IpAddr>().unwrap(), 30303).into(),
            PeerId::random(),
        ))
    }

    /// Checks a new node at `ip` against `existing`.
    fn accepts(limit: usize, existing: &[NodeEntry], ip: &str) -> bool {
        subnet_filter(&entry(ip).record, existing.iter(), limit)
    }

    #[test]
    fn table_filter_limits_public_ipv4_subnet() {
        let existing = (1..=MAX_NODES_PER_SUBNET_TABLE)
            .map(|i| entry(&format!("203.0.113.{i}")))
            .collect::<Vec<_>>();

        // The 10 nodes fit, an 11th in the same /24 does not.
        assert!(accepts(MAX_NODES_PER_SUBNET_TABLE, &existing[..9], "203.0.113.200"));
        assert!(!accepts(MAX_NODES_PER_SUBNET_TABLE, &existing, "203.0.113.200"));
        // Other /24s are unaffected.
        assert!(accepts(MAX_NODES_PER_SUBNET_TABLE, &existing, "203.0.114.1"));
        assert!(accepts(MAX_NODES_PER_SUBNET_TABLE, &existing, "198.51.100.1"));
    }

    #[test]
    fn bucket_filter_limits_public_ipv4_subnet() {
        let existing = [entry("203.0.113.1"), entry("203.0.113.2")];

        assert!(accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing[..1], "203.0.113.3"));
        assert!(!accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "203.0.113.3"));
        assert!(accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "203.0.114.3"));
    }

    #[test]
    fn filter_ignores_node_itself() {
        let existing = [entry("203.0.113.1"), entry("203.0.113.2")];
        let again = existing[0].clone();
        assert!(subnet_filter(&again.record, existing.iter(), MAX_NODES_PER_SUBNET_BUCKET));
    }

    #[test]
    fn non_public_addresses_are_never_limited() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "0.0.0.0",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.1.1",
        ] {
            let existing = (0..20).map(|_| entry(ip)).collect::<Vec<_>>();
            assert!(accepts(MAX_NODES_PER_SUBNET_TABLE, &existing, ip), "{ip}");
            assert!(accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, ip), "{ip}");
        }
    }

    #[test]
    fn ipv4_mapped_is_treated_as_ipv4() {
        let existing = [entry("::ffff:203.0.113.1"), entry("203.0.113.2")];

        // Mapped and plain IPv4 addresses share the /24.
        assert!(!accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "203.0.113.3"));
        assert!(!accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "::ffff:203.0.113.3"));
        assert!(accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "203.0.114.3"));
    }

    #[test]
    fn ipv6_subnets_are_48_bits() {
        let existing = [entry("2001:db8:1::1"), entry("2001:db8:1:ffff::2")];

        assert!(!accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "2001:db8:1:5::3"));
        assert!(accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "2001:db8:2::3"));
        // IPv4 and IPv6 never share a subnet.
        assert!(accepts(MAX_NODES_PER_SUBNET_BUCKET, &existing, "203.0.113.1"));
    }
}
