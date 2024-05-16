//! Tracks peer discovery for [`Discv5`](crate::Discv5).
use metrics::{Counter, Gauge};
use reth_metrics::Metrics;

use crate::NetworkStackId;

/// Information tracked by [`Discv5`](crate::Discv5).
#[derive(Debug, Default, Clone)]
pub struct Discv5Metrics {
    /// Frequency of networks advertised in discovered peers' node records.
    pub discovered_peers_advertised_networks: AdvertisedChainMetrics,
    /// Tracks discovered peers.
    pub discovered_peers: DiscoveredPeersMetrics,
}

/// Tracks discovered peers.
#[derive(Metrics, Clone)]
#[metrics(scope = "discv5")]
pub struct DiscoveredPeersMetrics {
    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Kbuckets
    ////////////////////////////////////////////////////////////////////////////////////////////////
    /// Total peers currently in [`discv5::Discv5`]'s kbuckets.
    kbucket_peers_raw_total: Gauge,
    /// Total discovered peers that are inserted into [`discv5::Discv5`]'s kbuckets.
    ///
    /// This is a subset of the total established sessions, in which all peers advertise a udp
    /// socket in their node record which is reachable from the local node. Only these peers make
    /// it into [`discv5::Discv5`]'s kbuckets and will hence be included in queries.
    ///
    /// Note: the definition of 'discovered' is not exactly synonymous in `reth_discv4::Discv4`.
    inserted_kbucket_peers_raw_total: Counter,

    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Sessions
    ////////////////////////////////////////////////////////////////////////////////////////////////
    /// Total peers currently connected to [`discv5::Discv5`].
    sessions_raw_total: Gauge,
    /// Total number of sessions established by [`discv5::Discv5`].
    established_sessions_raw_total: Counter,
    /// Total number of sessions established by [`discv5::Discv5`], with peers that don't advertise
    /// a socket which is reachable from the local node in their node record.
    ///
    /// These peers can't make it into [`discv5::Discv5`]'s kbuckets, and hence won't be part of
    /// queries (neither shared with peers in NODES responses, nor queried for peers with FINDNODE
    /// requests).
    established_sessions_unreachable_enr_total: Counter,
    /// Total number of sessions established by [`discv5::Discv5`], that pass configured
    /// [`filter`](crate::filter) rules.
    established_sessions_custom_filtered_total: Counter,
    /// Total number of unverifiable ENRs discovered by [`discv5::Discv5`].
    ///
    /// These are peers that fail [`discv5::Discv5`] session establishment, because the UDP socket
    /// they're making a connection from doesn't match the UDP socket advertised in their ENR.
    /// These peers will be denied a session (and hence can't make it into kbuckets) until they
    /// have update their ENR, to reflect their actual UDP socket.
    unverifiable_enrs_raw_total: Counter,
}

impl DiscoveredPeersMetrics {
    /// Sets current total number of peers in [`discv5::Discv5`]'s kbuckets.
    pub fn set_total_kbucket_peers(&self, num: usize) {
        self.kbucket_peers_raw_total.set(num as f64)
    }

    /// Increments the number of kbucket insertions in [`discv5::Discv5`].
    pub fn increment_kbucket_insertions(&self, num: u64) {
        self.inserted_kbucket_peers_raw_total.increment(num)
    }

    /// Sets current total number of peers connected to [`discv5::Discv5`].
    pub fn set_total_sessions(&self, num: usize) {
        self.sessions_raw_total.set(num as f64)
    }

    /// Increments number of sessions established by [`discv5::Discv5`].
    pub fn increment_established_sessions_raw(&self, num: u64) {
        self.established_sessions_raw_total.increment(num)
    }

    /// Increments number of sessions established by [`discv5::Discv5`], with peers that don't have
    /// a reachable node record.
    pub fn increment_established_sessions_unreachable_enr(&self, num: u64) {
        self.established_sessions_unreachable_enr_total.increment(num)
    }

    /// Increments number of sessions established by [`discv5::Discv5`], that pass configured
    /// [`filter`](crate::filter) rules.
    pub fn increment_established_sessions_filtered(&self, num: u64) {
        self.established_sessions_custom_filtered_total.increment(num)
    }

    /// Increments number of unverifiable ENRs discovered by [`discv5::Discv5`]. These are peers
    /// that fail session establishment because their advertised UDP socket doesn't match the
    /// socket they are making the connection from.
    pub fn increment_unverifiable_enrs_raw_total(&self, num: u64) {
        self.unverifiable_enrs_raw_total.increment(num)
    }
}

/// Tracks frequency of networks that are advertised by discovered peers.
///
/// Peers advertise the chain they belong to as a kv-pair in their node record, using the network
/// as key.
#[derive(Metrics, Clone)]
#[metrics(scope = "discv5")]
pub struct AdvertisedChainMetrics {
    /// Frequency of node records with a kv-pair with [`OPEL`](NetworkStackId::OPEL) as
    /// key.
    opel: Counter,

    /// Frequency of node records with a kv-pair with [`OPSTACK`](NetworkStackId::OPSTACK) as
    /// key.
    opstack: Counter,

    /// Frequency of node records with a kv-pair with [`ETH`](NetworkStackId::ETH) as key.
    eth: Counter,

    /// Frequency of node records with a kv-pair with [`ETH2`](NetworkStackId::ETH2) as key.
    eth2: Counter,
}

impl AdvertisedChainMetrics {
    /// Counts each recognised network stack type that is advertised on node record, once.
    pub fn increment_once_by_network_type(&self, enr: &discv5::Enr) {
        if enr.get_raw_rlp(NetworkStackId::OPEL).is_some() {
            self.opel.increment(1u64)
        }
        if enr.get_raw_rlp(NetworkStackId::OPSTACK).is_some() {
            self.opstack.increment(1u64)
        }
        if enr.get_raw_rlp(NetworkStackId::ETH).is_some() {
            self.eth.increment(1u64)
        }
        if enr.get_raw_rlp(NetworkStackId::ETH2).is_some() {
            self.eth2.increment(1u64)
        }
    }
}
