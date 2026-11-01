//! Per-subnet limits for the discv4 routing table.

use crate::NodeEntry;
use discv5::kbucket::Filter;
use std::net::IpAddr;

/// Maximum number of nodes from the same subnet in the whole table.
pub(crate) const MAX_NODES_PER_SUBNET_TABLE: usize = 10;

/// Maximum number of nodes from the same subnet in a single bucket.
pub(crate) const MAX_NODES_PER_SUBNET_BUCKET: usize = 2;

/// Limits the number of nodes sharing an IPv4 /24 or IPv6 /48 subnet in the whole table.
///
/// Non-public addresses are never limited, see [`subnet`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SubnetTableFilter;

impl Filter<NodeEntry> for SubnetTableFilter {
    fn filter(
        &self,
        value_to_be_inserted: &NodeEntry,
        other_vals: &mut dyn Iterator<Item = &NodeEntry>,
    ) -> bool {
        subnet_filter(value_to_be_inserted, other_vals, MAX_NODES_PER_SUBNET_TABLE)
    }
}

/// Limits the number of nodes sharing an IPv4 /24 or IPv6 /48 subnet in a single bucket.
///
/// Non-public addresses are never limited, see [`subnet`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SubnetBucketFilter;

impl Filter<NodeEntry> for SubnetBucketFilter {
    fn filter(
        &self,
        value_to_be_inserted: &NodeEntry,
        other_vals: &mut dyn Iterator<Item = &NodeEntry>,
    ) -> bool {
        subnet_filter(value_to_be_inserted, other_vals, MAX_NODES_PER_SUBNET_BUCKET)
    }
}

/// The network prefix that nodes are grouped by: IPv4 /24 or IPv6 /48.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subnet {
    V4([u8; 3]),
    V6([u8; 6]),
}

/// Returns `false` if inserting `value_to_be_inserted` would put more than `limit` other nodes of
/// the same subnet next to it.
fn subnet_filter(
    value_to_be_inserted: &NodeEntry,
    other_vals: &mut dyn Iterator<Item = &NodeEntry>,
    limit: usize,
) -> bool {
    let Some(target) = subnet(value_to_be_inserted.record.address) else { return true };

    let count = other_vals
        // the node itself does not count against its own subnet.
        .filter(|other| other.record.id != value_to_be_inserted.record.id)
        .filter(|other| subnet(other.record.address) == Some(target))
        .take(limit)
        .count();

    count < limit
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
        NodeEntry::new(NodeRecord::new(
            (ip.parse::<IpAddr>().unwrap(), 30303).into(),
            PeerId::random(),
        ))
    }

    /// Runs `filter` for a new node at `ip` against `existing`.
    fn accepts(filter: &impl Filter<NodeEntry>, existing: &[NodeEntry], ip: &str) -> bool {
        filter.filter(&entry(ip), &mut existing.iter())
    }

    #[test]
    fn table_filter_limits_public_ipv4_subnet() {
        let existing: Vec<_> =
            (1..=MAX_NODES_PER_SUBNET_TABLE).map(|i| entry(&format!("203.0.113.{i}"))).collect();

        // the 10 nodes fit, an 11th in the same /24 does not.
        assert!(accepts(&SubnetTableFilter, &existing[..9], "203.0.113.200"));
        assert!(!accepts(&SubnetTableFilter, &existing, "203.0.113.200"));
        // other /24s are unaffected.
        assert!(accepts(&SubnetTableFilter, &existing, "203.0.114.1"));
        assert!(accepts(&SubnetTableFilter, &existing, "198.51.100.1"));
    }

    #[test]
    fn bucket_filter_limits_public_ipv4_subnet() {
        let existing = [entry("203.0.113.1"), entry("203.0.113.2")];

        assert!(accepts(&SubnetBucketFilter, &existing[..1], "203.0.113.3"));
        assert!(!accepts(&SubnetBucketFilter, &existing, "203.0.113.3"));
        assert!(accepts(&SubnetBucketFilter, &existing, "203.0.114.3"));
    }

    #[test]
    fn filter_ignores_node_itself() {
        let existing = [entry("203.0.113.1"), entry("203.0.113.2")];
        let again = existing[0].clone();
        assert!(SubnetBucketFilter.filter(&again, &mut existing.iter()));
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
            let existing: Vec<_> = (0..20).map(|_| entry(ip)).collect();
            assert!(accepts(&SubnetTableFilter, &existing, ip), "{ip}");
            assert!(accepts(&SubnetBucketFilter, &existing, ip), "{ip}");
        }
    }

    #[test]
    fn ipv4_mapped_is_treated_as_ipv4() {
        let existing = [entry("::ffff:203.0.113.1"), entry("203.0.113.2")];

        // mapped and plain IPv4 addresses share the /24.
        assert!(!accepts(&SubnetBucketFilter, &existing, "203.0.113.3"));
        assert!(!accepts(&SubnetBucketFilter, &existing, "::ffff:203.0.113.3"));
        assert!(accepts(&SubnetBucketFilter, &existing, "203.0.114.3"));
    }

    #[test]
    fn ipv6_subnets_are_48_bits() {
        let existing = [entry("2001:db8:1::1"), entry("2001:db8:1:ffff::2")];

        assert!(!accepts(&SubnetBucketFilter, &existing, "2001:db8:1:5::3"));
        assert!(accepts(&SubnetBucketFilter, &existing, "2001:db8:2::3"));
        // IPv4 and IPv6 never share a subnet.
        assert!(accepts(&SubnetBucketFilter, &existing, "203.0.113.1"));
    }
}
