use crate::node::NodeRecord;
use reth_net_common::ban_list::BanList;
///! A set of configuration parameters to tune the discovery protocol.
// This basis of this file has been taken from the discv5 codebase:
// https://github.com/sigp/discv5
use std::collections::HashSet;
use std::time::Duration;

/// Configuration parameters that define the performance of the discovery network.
#[derive(Clone, Debug)]
pub struct Discv4Config {
    /// Whether to enable the incoming packet filter. Default: false.
    pub enable_packet_filter: bool,
    /// The number of retries for each UDP request. Default: 1.
    pub request_retries: u8,
    /// The time between pings to ensure connectivity amongst connected nodes. Default: 300
    /// seconds.
    pub ping_interval: Duration,
    /// The duration of we consider a ping timed out.
    pub ping_timeout: Duration,
    /// The rate at which lookups should be triggered.
    pub lookup_interval: Duration,
    /// The duration of we consider a FindNode request timed out.
    pub find_node_timeout: Duration,
    /// The duration we set for neighbours responses
    pub neighbours_timeout: Duration,
    /// Provides a way to ban peers and ips.
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
}

impl Discv4Config {
    /// Returns a new default builder instance
    pub fn builder() -> Discv4ConfigBuilder {
        Default::default()
    }
}

impl Default for Discv4Config {
    fn default() -> Self {
        Self {
            enable_packet_filter: false,
            request_retries: 1,
            ping_interval: Duration::from_secs(300),
            ping_timeout: Duration::from_secs(5),
            find_node_timeout: Duration::from_secs(2),
            neighbours_timeout: Duration::from_secs(30),
            lookup_interval: Duration::from_secs(20),
            ban_list: Default::default(),
            ban_duration: Some(Duration::from_secs(3600)), // 1 hour
            bootstrap_nodes: Default::default(),
            enable_dht_random_walk: true,
            enable_lookup: true,
        }
    }
}

/// Builder type for [`Discv4Config`]
#[derive(Debug, Default)]
pub struct Discv4ConfigBuilder {
    config: Discv4Config,
}

impl Discv4ConfigBuilder {
    /// Whether to enable the incoming packet filter.
    pub fn enable_packet_filter(&mut self) -> &mut Self {
        self.config.enable_packet_filter = true;
        self
    }

    /// The number of retries for each UDP request.
    pub fn request_retries(&mut self, retries: u8) -> &mut Self {
        self.config.request_retries = retries;
        self
    }

    /// The time between pings to ensure connectivity amongst connected nodes.
    pub fn ping_interval(&mut self, interval: Duration) -> &mut Self {
        self.config.ping_interval = interval;
        self
    }

    /// Sets the timeout for pings
    pub fn ping_timeout(&mut self, duration: Duration) -> &mut Self {
        self.config.ping_timeout = duration;
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
