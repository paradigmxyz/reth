//! A set of configuration parameters to tune the discovery protocol.
//!
//! This basis of this file has been taken from the discv5 codebase:
//! <https://github.com/sigp/discv5>

use reth_net_common::ban_list::BanList;
use reth_net_nat::{NatResolver, ResolveNatInterval};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    NodeRecord,
};
use reth_rlp::Encodable;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Configuration parameters that define the performance of the discovery network.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Discv4Config {
    /// Whether to enable the incoming packet filter. Default: false.
    pub enable_packet_filter: bool,
    /// Size of the channel buffer for outgoing messages.
    pub udp_egress_message_buffer: usize,
    /// Size of the channel buffer for incoming messages.
    pub udp_ingress_message_buffer: usize,
    /// The number of allowed failures for `FindNode` requests. Default: 5.
    pub max_find_node_failures: u8,
    /// The interval to use when checking for expired nodes that need to be re-pinged. Default:
    /// 300sec, 5min.
    pub ping_interval: Duration,
    /// The duration of we consider a ping timed out.
    pub ping_expiration: Duration,
    /// The rate at which lookups should be triggered.
    pub lookup_interval: Duration,
    /// The duration of we consider a FindNode request timed out.
    pub request_timeout: Duration,
    /// The duration after which we consider an enr request timed out.
    pub enr_expiration: Duration,
    /// The duration we set for neighbours responses
    pub neighbours_expiration: Duration,
    /// Provides a way to ban peers and ips.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub ban_list: BanList,
    /// Set the default duration for which nodes are banned for. This timeouts are checked every 5
    /// minutes, so the precision will be to the nearest 5 minutes. If set to `None`, bans from
    /// the filter will last indefinitely. Default is 1 hour.
    pub ban_duration: Option<Duration>,
    /// Nodes to boot from.
    pub bootstrap_nodes: HashSet<NodeRecord>,
    /// Whether to randomly discover new peers.
    ///
    /// If true, the node will automatically randomly walk the DHT in order to find new peers.
    pub enable_dht_random_walk: bool,
    /// Whether to automatically lookup peers.
    pub enable_lookup: bool,
    /// Whether to enforce EIP-868 extension
    pub enable_eip868: bool,
    /// Whether to respect expiration timestamps in messages
    pub enforce_expiration_timestamps: bool,
    /// Additional pairs to include in The [`Enr`](enr::Enr) if EIP-868 extension is enabled <https://eips.ethereum.org/EIPS/eip-868>
    pub additional_eip868_rlp_pairs: HashMap<Vec<u8>, Bytes>,
    /// If configured, try to resolve public ip
    pub external_ip_resolver: Option<NatResolver>,
    /// If configured and a `external_ip_resolver` is configured, try to resolve the external ip
    /// using this interval.
    pub resolve_external_ip_interval: Option<Duration>,
}

impl Discv4Config {
    /// Returns a new default builder instance
    pub fn builder() -> Discv4ConfigBuilder {
        Default::default()
    }

    /// Add another key value pair to include in the ENR
    pub fn add_eip868_pair(&mut self, key: impl AsRef<[u8]>, value: impl Encodable) -> &mut Self {
        let mut buf = BytesMut::new();
        value.encode(&mut buf);
        self.add_eip868_rlp_pair(key, buf.freeze())
    }

    /// Add another key value pair to include in the ENR
    pub fn add_eip868_rlp_pair(&mut self, key: impl AsRef<[u8]>, rlp: Bytes) -> &mut Self {
        self.additional_eip868_rlp_pairs.insert(key.as_ref().to_vec(), rlp);
        self
    }

    /// Extend additional key value pairs to include in the ENR
    pub fn extend_eip868_rlp_pairs(
        &mut self,
        pairs: impl IntoIterator<Item = (impl AsRef<[u8]>, Bytes)>,
    ) -> &mut Self {
        for (k, v) in pairs.into_iter() {
            self.add_eip868_rlp_pair(k, v);
        }
        self
    }

    /// Returns the corresponding [`ResolveNatInterval`], if a [NatResolver] and an interval was
    /// configured
    pub fn resolve_external_ip_interval(&self) -> Option<ResolveNatInterval> {
        let resolver = self.external_ip_resolver?;
        let interval = self.resolve_external_ip_interval?;
        Some(ResolveNatInterval::interval(resolver, interval))
    }
}

impl Default for Discv4Config {
    fn default() -> Self {
        Self {
            enable_packet_filter: false,
            /// This should be high enough to cover an entire recursive FindNode lookup which is
            /// includes sending FindNode to nodes it discovered in the rounds using the
            /// concurrency factor ALPHA
            udp_egress_message_buffer: 1024,
            /// Every outgoing request will eventually lead to an incoming response
            udp_ingress_message_buffer: 1024,
            max_find_node_failures: 5,
            ping_interval: Duration::from_secs(300),
            /// unified expiration and timeout durations, mirrors geth's `expiration` duration
            ping_expiration: Duration::from_secs(20),
            enr_expiration: Duration::from_secs(20),
            neighbours_expiration: Duration::from_secs(20),
            request_timeout: Duration::from_secs(20),

            lookup_interval: Duration::from_secs(20),
            ban_list: Default::default(),
            ban_duration: Some(Duration::from_secs(3600)), // 1 hour
            bootstrap_nodes: Default::default(),
            enable_dht_random_walk: true,
            enable_lookup: true,
            enable_eip868: true,
            enforce_expiration_timestamps: true,
            additional_eip868_rlp_pairs: Default::default(),
            external_ip_resolver: Some(Default::default()),
            /// By default retry public IP using a 5min interval
            resolve_external_ip_interval: Some(Duration::from_secs(60 * 5)),
        }
    }
}

/// Builder type for [`Discv4Config`]
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Discv4ConfigBuilder {
    config: Discv4Config,
}

impl Discv4ConfigBuilder {
    /// Whether to enable the incoming packet filter.
    pub fn enable_packet_filter(&mut self) -> &mut Self {
        self.config.enable_packet_filter = true;
        self
    }

    /// Sets the channel size for incoming messages
    pub fn udp_ingress_message_buffer(&mut self, udp_ingress_message_buffer: usize) -> &mut Self {
        self.config.udp_ingress_message_buffer = udp_ingress_message_buffer;
        self
    }

    /// Sets the channel size for outgoing messages
    pub fn udp_egress_message_buffer(&mut self, udp_egress_message_buffer: usize) -> &mut Self {
        self.config.udp_egress_message_buffer = udp_egress_message_buffer;
        self
    }

    /// The number of allowed request failures for `findNode` requests.
    pub fn max_find_node_failures(&mut self, max_find_node_failures: u8) -> &mut Self {
        self.config.max_find_node_failures = max_find_node_failures;
        self
    }

    /// The time between pings to ensure connectivity amongst connected nodes.
    pub fn ping_interval(&mut self, interval: Duration) -> &mut Self {
        self.config.ping_interval = interval;
        self
    }

    /// Sets the timeout after which requests are considered timed out
    pub fn request_timeout(&mut self, duration: Duration) -> &mut Self {
        self.config.request_timeout = duration;
        self
    }

    /// Sets the expiration duration for pings
    pub fn ping_expiration(&mut self, duration: Duration) -> &mut Self {
        self.config.ping_expiration = duration;
        self
    }

    /// Sets the expiration duration for enr requests
    pub fn enr_request_expiration(&mut self, duration: Duration) -> &mut Self {
        self.config.enr_expiration = duration;
        self
    }

    /// Whether to discover random nodes in the DHT.
    pub fn enable_dht_random_walk(&mut self, enable_dht_random_walk: bool) -> &mut Self {
        self.config.enable_dht_random_walk = enable_dht_random_walk;
        self
    }

    /// Whether to automatically lookup
    pub fn enable_lookup(&mut self, enable_lookup: bool) -> &mut Self {
        self.config.enable_lookup = enable_lookup;
        self
    }

    /// Whether to enforce expiration timestamps in messages.
    pub fn enable_eip868(&mut self, enable_eip868: bool) -> &mut Self {
        self.config.enable_eip868 = enable_eip868;
        self
    }

    /// Whether to enable EIP-868
    pub fn enforce_expiration_timestamps(
        &mut self,
        enforce_expiration_timestamps: bool,
    ) -> &mut Self {
        self.config.enforce_expiration_timestamps = enforce_expiration_timestamps;
        self
    }

    /// Add another key value pair to include in the ENR
    pub fn add_eip868_pair(&mut self, key: impl AsRef<[u8]>, value: impl Encodable) -> &mut Self {
        let mut buf = BytesMut::new();
        value.encode(&mut buf);
        self.add_eip868_rlp_pair(key, buf.freeze())
    }

    /// Add another key value pair to include in the ENR
    pub fn add_eip868_rlp_pair(&mut self, key: impl AsRef<[u8]>, rlp: Bytes) -> &mut Self {
        self.config.additional_eip868_rlp_pairs.insert(key.as_ref().to_vec(), rlp);
        self
    }

    /// Extend additional key value pairs to include in the ENR
    pub fn extend_eip868_rlp_pairs(
        &mut self,
        pairs: impl IntoIterator<Item = (impl AsRef<[u8]>, Bytes)>,
    ) -> &mut Self {
        for (k, v) in pairs.into_iter() {
            self.add_eip868_rlp_pair(k, v);
        }
        self
    }

    /// A set of lists that can ban IP's or PeerIds from the server. See
    /// [`BanList`].
    pub fn ban_list(&mut self, ban_list: BanList) -> &mut Self {
        self.config.ban_list = ban_list;
        self
    }

    /// Sets the lookup interval duration.
    pub fn lookup_interval(&mut self, lookup_interval: Duration) -> &mut Self {
        self.config.lookup_interval = lookup_interval;
        self
    }

    /// Set the default duration for which nodes are banned for. This timeouts are checked every 5
    /// minutes, so the precision will be to the nearest 5 minutes. If set to `None`, bans from
    /// the filter will last indefinitely. Default is 1 hour.
    pub fn ban_duration(&mut self, ban_duration: Option<Duration>) -> &mut Self {
        self.config.ban_duration = ban_duration;
        self
    }

    /// Adds a boot node
    pub fn add_boot_node(&mut self, node: NodeRecord) -> &mut Self {
        self.config.bootstrap_nodes.insert(node);
        self
    }

    /// Adds multiple boot nodes
    pub fn add_boot_nodes(&mut self, nodes: impl IntoIterator<Item = NodeRecord>) -> &mut Self {
        self.config.bootstrap_nodes.extend(nodes);
        self
    }

    /// Configures if and how the external IP of the node should be resolved.
    pub fn external_ip_resolver(&mut self, external_ip_resolver: Option<NatResolver>) -> &mut Self {
        self.config.external_ip_resolver = external_ip_resolver;
        self
    }

    /// Sets the interval at which the external IP is to be resolved.
    pub fn resolve_external_ip_interval(
        &mut self,
        resolve_external_ip_interval: Option<Duration>,
    ) -> &mut Self {
        self.config.resolve_external_ip_interval = resolve_external_ip_interval;
        self
    }

    /// Returns the configured [`Discv4Config`]
    pub fn build(&self) -> Discv4Config {
        self.config.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let mut builder = Discv4Config::builder();
        let _ = builder
            .enable_lookup(true)
            .enable_dht_random_walk(true)
            .add_boot_nodes(HashSet::new())
            .ban_duration(None)
            .lookup_interval(Duration::from_secs(3))
            .enable_lookup(true)
            .build();
    }
}
