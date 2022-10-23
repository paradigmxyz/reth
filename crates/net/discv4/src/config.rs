use discv5::PermitBanList;
///! A set of configuration parameters to tune the discovery protocol.
// This basis of this file has been taken from the discv4 codebase:
// https://github.com/sigp/discv4
use std::time::Duration;

/// Configuration parameters that define the performance of the discovery network.
#[derive(Clone, Debug)]
pub struct Discv4Config {
    /// Whether to enable the incoming packet filter. Default: false.
    pub enable_packet_filter: bool,

    /// The request timeout for each UDP request. Default: 1 seconds.
    pub request_timeout: Duration,

    /// The timeout after which a `QueryPeer` in an ongoing query is marked unresponsive.
    /// Unresponsive peers don't count towards the parallelism limits for a query.
    /// Hence, we may potentially end up making more requests to good peers. Default: 2 seconds.
    pub query_peer_timeout: Duration,

    /// The timeout for an entire query. Any peers discovered for this query are returned. Default
    /// 60 seconds.
    pub query_timeout: Duration,

    /// The number of retries for each UDP request. Default: 1.
    pub request_retries: u8,

    /// The session timeout for each node. Default: 1 day.
    pub session_timeout: Duration,

    /// The time between pings to ensure connectivity amongst connected nodes. Default: 300
    /// seconds.
    pub ping_interval: Duration,

    /// Reports all discovered ENR's when traversing the DHT to the event stream. Default true.
    pub report_discovered_peers: bool,

    /// A set of lists that permit or ban IP's or NodeIds from the server. See
    /// `crate::PermitBanList`.
    pub permit_ban_list: PermitBanList,

    /// Set the default duration for which nodes are banned for. This timeouts are checked every 5
    /// minutes, so the precision will be to the nearest 5 minutes. If set to `None`, bans from
    /// the filter will last indefinitely. Default is 1 hour.
    pub ban_duration: Option<Duration>,
}

impl Discv4Config {
    /// Returns a new default builder isntance
    pub fn builder() -> Discv4ConfigBuilder {
        Default::default()
    }
}

impl Default for Discv4Config {
    fn default() -> Self {
        Self {
            enable_packet_filter: false,
            request_timeout: Duration::from_secs(1),
            query_peer_timeout: Duration::from_secs(2),
            query_timeout: Duration::from_secs(60),
            request_retries: 1,
            session_timeout: Duration::from_secs(86400),
            ping_interval: Duration::from_secs(300),
            report_discovered_peers: true,
            permit_ban_list: PermitBanList::default(),
            ban_duration: Some(Duration::from_secs(3600)), // 1 hour
        }
    }
}

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

    /// The request timeout for each UDP request.
    pub fn request_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.config.request_timeout = timeout;
        self
    }

    /// The timeout after which a `QueryPeer` in an ongoing query is marked unresponsive.
    /// Unresponsive peers don't count towards the parallelism limits for a query.
    /// Hence, we may potentially end up making more requests to good peers.
    pub fn query_peer_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.config.query_peer_timeout = timeout;
        self
    }

    /// The timeout for an entire query. Any peers discovered before this timeout are returned.
    pub fn query_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.config.query_timeout = timeout;
        self
    }

    /// The number of retries for each UDP request.
    pub fn request_retries(&mut self, retries: u8) -> &mut Self {
        self.config.request_retries = retries;
        self
    }

    /// The session timeout for each node.
    pub fn session_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.config.session_timeout = timeout;
        self
    }

    /// The time between pings to ensure connectivity amongst connected nodes.
    pub fn ping_interval(&mut self, interval: Duration) -> &mut Self {
        self.config.ping_interval = interval;
        self
    }

    /// Disables reporting of discovered peers through the event stream.
    pub fn disable_report_discovered_peers(&mut self) -> &mut Self {
        self.config.report_discovered_peers = false;
        self
    }

    /// A set of lists that permit or ban IP's or NodeIds from the server. See
    /// `crate::PermitBanList`.
    pub fn permit_ban_list(&mut self, list: PermitBanList) -> &mut Self {
        self.config.permit_ban_list = list;
        self
    }

    /// Set the default duration for which nodes are banned for. This timeouts are checked every 5
    /// minutes, so the precision will be to the nearest 5 minutes. If set to `None`, bans from
    /// the filter will last indefinitely. Default is 1 hour.
    pub fn ban_duration(&mut self, ban_duration: Option<Duration>) -> &mut Self {
        self.config.ban_duration = ban_duration;
        self
    }

    pub fn build(&mut self) -> Discv4Config {
        self.config.clone()
    }
}
