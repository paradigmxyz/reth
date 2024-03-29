//! Tracks peer discovery for [`Discv5`](crate::Discv5).
use metrics::{Counter, Gauge};
use reth_metrics::Metrics;

use crate::config::{ETH, ETH2, OPSTACK};

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
    total_kbucket_peers_raw: Gauge,
    /// Total discovered peers that are inserted into [`discv5::Discv5`]'s kbuckets.
    ///
    /// This is a subset of the total established sessions, in which all peers advertise a udp
    /// socket in their node record which is reachable from the local node. Only these peers make
    /// it into [`discv5::Discv5`]'s kbuckets and will hence be included in queries.
    ///
    /// Note: the definition of 'discovered' is not exactly synonymous in `reth_discv4::Discv4`.
    total_inserted_kbucket_peers_raw: Counter,

    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Sessions
    ////////////////////////////////////////////////////////////////////////////////////////////////
    /// Total peers currently connected to [`discv5::Discv5`].
    total_sessions_raw: Gauge,
    /// Total number of sessions established by [`discv5::Discv5`].
    total_established_sessions_raw: Counter,
    /// Total number of sessions established by [`discv5::Discv5`], with peers that don't advertise
    /// a socket which is reachable from the local node in their node record.
    ///
    /// These peers can't make it into [`discv5::Discv5`]'s kbuckets, and hence won't be part of
    /// queries (neither shared with peers in NODES responses, nor queried for peers with FINDNODE
    /// requests).
    total_established_sessions_unreachable_enr: Counter,
    /// Total number of sessions established by [`discv5::Discv5`], that pass configured
    /// [`filter`](crate::filter) rules.
    total_established_sessions_custom_filtered: Counter,
}

impl DiscoveredPeersMetrics {
    /// Sets current total number of peers in [`discv5::Discv5`]'s kbuckets.
    pub fn set_total_kbucket_peers(&mut self, num: usize) {
        self.total_kbucket_peers_raw.set(num as f64)
    }

    /// Increments the number of kbucket insertions in [`discv5::Discv5`].
    pub fn increment_kbucket_insertions(&mut self, num: u64) {
        self.total_inserted_kbucket_peers_raw.increment(num)
    }

    /// Sets current total number of peers connected to [`discv5::Discv5`].
    pub fn set_total_sessions(&mut self, num: usize) {
        self.total_sessions_raw.set(num as f64)
    }

    /// Increments number of sessions established by [`discv5::Discv5`].
    pub fn increment_established_sessions_raw(&mut self, num: u64) {
        self.total_established_sessions_raw.increment(num)
    }

    /// Increments number of sessions established by [`discv5::Discv5`], with peers that don't have
    /// a reachable node record.
    pub fn increment_established_sessions_unreachable_enr(&mut self, num: u64) {
        self.total_established_sessions_unreachable_enr.increment(num)
    }

    /// Increments number of sessions established by [`discv5::Discv5`], that pass configured
    /// [`filter`](crate::filter) rules.
    pub fn increment_established_sessions_filtered(&mut self, num: u64) {
        self.total_established_sessions_custom_filtered.increment(num)
    }
}

/// Tracks frequency of networks that are advertised by discovered peers.
///
/// Peers advertise the chain they belong to as a kv-pair in their node record, using the network
/// as key.
#[derive(Metrics, Clone)]
#[metrics(scope = "discv5")]
pub struct AdvertisedChainMetrics {
    /// Frequency of node records with a kv-pair with [`OPSTACK`] as key.
    opstack: Counter,

    /// Frequency of node records with a kv-pair with [`ETH`] as key.
    eth: Counter,

    /// Frequency of node records with a kv-pair with [`ETH2`] as key.
    eth2: Counter,
}

impl AdvertisedChainMetrics {
    /// Counts each recognised network type that is advertised on node record, once.
    pub fn increment_once_by_network_type(&mut self, enr: &discv5::Enr) {
        if enr.get_raw_rlp(OPSTACK).is_some() {
            self.opstack.increment(1u64)
        }
        if enr.get_raw_rlp(ETH).is_some() {
            self.eth.increment(1u64)
        }
        if enr.get_raw_rlp(ETH2).is_some() {
            self.eth2.increment(1u64)
        }
    }
}
