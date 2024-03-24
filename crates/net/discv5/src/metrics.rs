//! Tracks peer discovery for [`Discv5`](crate::Discv5) and
//! [`Discv5BCv4`](crate::Discv5BCv4`).

use metrics::Counter;
use reth_metrics::Metrics;

use crate::{ChainRef, IdentifyForkIdKVPair};

#[doc(hidden)]
pub trait UpdateMetrics {
    fn with_metrics<R, F>(&mut self, f: F) -> R
    where
        F: Fn(&mut Metrics) -> R;
}

/// Information tracked by [`Discv5`](crate::Discv5) and
/// [`Discv5BCv4`](crate::Discv5BCv4).
#[derive(Debug, Default, Clone)]
pub struct Metrics {
    /// Frequency of chains advertised in discovered peers' node records.
    pub discovered_peers_chain_type: AdvertisedChainMetrics,
    /// Tracks discovered peers by protocol version.
    pub discovered_peers_by_protocol: DiscoveredPeersMetrics,
}

/// Tracks discovered peers by protocol version, that pass the configured
/// [`FilterDiscovered`](crate::filter::FilterDiscovered).
#[derive(Metrics, Clone)]
#[metrics(scope = "discv5")]
pub struct DiscoveredPeersMetrics {
    /// Total peers discovered by [`discv5::Discv5`].
    ///
    /// Note: the definition of 'discovered' is not exactly synonymous in [`discv5::Discv5`] and
    /// [`Discv4`](reth_discv4::Discv4).
    total_discovered_peers_discv5: Counter,
    /// Increments number of peers discovered by [`discv5::Discv5`] as part of a query, which local
    /// node is not able to make an outgoing connection to, as there is no socket advertised in
    /// peer's node record that is reachable from the local node.
    total_discovered_unreachable_v5: Counter,
    /// Total number of sessions that [`discv5::Discv5`] has established with peers with
    /// unreachable node records. Naturally, these will all be incoming sessions. These peers will
    /// never end up in [`discv5::Discv5`]'s kbuckets, so will never be included in queries.
    total_sessions_unreachable_v5: Counter,
    /// Total peers discovered by [`Discv4`](reth_discv4::Discv4), running as service to support
    /// backwards compatibility (not running at full capacity) in
    /// [`Discv5BCv4`](crate::Discv5BCv4).
    ///
    /// Note: the definition of 'discovered' is not exactly synonymous in [`discv5::Discv5`] and
    /// [`Discv4`](reth_discv4::Discv4).
    total_discovered_peers_discv4_as_downgrade: Counter,
}

impl DiscoveredPeersMetrics {
    /// Increments peers discovered by [`discv5::Discv5`].
    pub fn increment_discovered_v5(&mut self, num: u64) {
        self.total_discovered_peers_discv5.increment(num)
    }

    /// Increments number of peers discovered by [`discv5::Discv5`] as part of a query, which local
    /// node is not able to make an outgoing connection to, as there is no socket advertised in
    /// peer's node record that is reachable from the local node.
    pub fn increment_discovered_unreachable_v5(&mut self, num: u64) {
        self.total_discovered_unreachable_v5.increment(num)
    }

    /// Increments the number of sessions that [`discv5::Discv5`] has established with peers with
    /// unreachable node records.
    pub fn increment_sessions_unreachable_v5(&mut self, num: u64) {
        self.total_sessions_unreachable_v5.increment(num)
    }

    /// Counts peers discovered by [`Discv4`](reth_discv4::Discv4) as backwards compatibility
    /// support in [`Discv5BCv4`](crate::Discv5BCv4).
    pub fn increment_discovered_v4_as_downgrade(&mut self, num: u64) {
        self.total_discovered_peers_discv4_as_downgrade.increment(num)
    }
}

/// Tracks frequency of chains that are advertised by discovered peers. Peers advertise the chain
/// they belong to in their node record. Indirectly measure number of unusable discovered peers,
/// since, if a node advertises both 'eth' and 'eth2', it will be counted as advertising 'eth' not
/// 'eth2'.
#[derive(Metrics, Clone)]
#[metrics(scope = "discv5")]
pub struct AdvertisedChainMetrics {
    /// Frequency of node records with a kv-pair with [`OPSTACK`](IdentifyForkIdKVPair::OPSTACK) as
    /// key.
    opstack: Counter,

    /// Frequency of node records with a kv-pair with [`ETH`](IdentifyForkIdKVPair::ETH) as key.
    eth: Counter,

    /// Frequency of node records with a kv-pair with [`ETH2`](IdentifyForkIdKVPair::ETH2) as key.
    eth2: Counter,
}

impl AdvertisedChainMetrics {
    /// Counts each recognised chain type that is advertised on node record, once.
    pub fn increment_once_by_chain_type(&mut self, counter: AdvertisedChainCounter) {
        let AdvertisedChainCounter { opstack, eth, eth2 } = counter;

        if opstack {
            self.opstack.increment(1u64)
        }
        if eth {
            self.eth.increment(1u64)
        }
        if eth2 {
            self.eth2.increment(1u64)
        }
    }
}

/// Aggregates chain type data from one node record.
#[derive(Debug, Default)]
pub struct AdvertisedChainCounter {
    opstack: bool,
    eth: bool,
    eth2: bool,
}

impl AdvertisedChainCounter {
    /// Counts each recognised chain type that is advertised on node record, once.
    pub fn increment_once_by_chain_type(&mut self, enr: &discv5::Enr) {
        if enr.get_raw_rlp(ChainRef::OPSTACK).is_some() {
            self.opstack = true
        }

        if enr.get_raw_rlp(ChainRef::ETH).is_some() {
            self.eth = true
        }

        if enr.get_raw_rlp(ChainRef::ETH2).is_some() {
            self.eth2 = true
        }
    }
}
