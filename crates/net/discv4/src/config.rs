use discv5::{
    kbucket::MAX_NODES_PER_BUCKET, IpMode, PermitBanList, RateLimiter, RateLimiterBuilder,
};
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

    /// The interval over which votes are remembered when determining our external IP. A lower
    /// interval will respond faster to IP changes. Default is 30 seconds.
    pub vote_duration: Duration,

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

    /// The maximum number of established sessions to maintain. Default: 1000.
    pub session_cache_capacity: usize,

    /// Limits the number of IP addresses from the same
    /// /24 subnet in the kbuckets table. This is to mitigate eclipse attacks. Default: false.
    pub ip_limit: bool,

    /// Sets a maximum limit to the number of  incoming nodes (nodes that have dialed us) to exist
    /// per-bucket. This cannot be larger than the bucket size (16). By default this is
    /// disabled (set to the maximum bucket size, 16).
    pub incoming_bucket_limit: usize,

    /// The time between pings to ensure connectivity amongst connected nodes. Default: 300
    /// seconds.
    pub ping_interval: Duration,

    /// Configures the type of socket to bind to. This also affects the selection of address to use
    /// to contact an ENR.
    pub ip_mode: IpMode,

    /// Reports all discovered ENR's when traversing the DHT to the event stream. Default true.
    pub report_discovered_peers: bool,

    /// A set of configuration parameters for setting inbound request rate limits. See
    /// [`RateLimiterBuilder`] for options. This is only functional if the packet filter is
    /// enabled via the `enable_packet_filter` option. See the `Default` implementation for
    /// default values. If set to None, inbound requests are not filtered.
    pub filter_rate_limiter: Option<RateLimiter>,

    /// The maximum number of node-ids allowed per IP address before the IP address gets banned.
    /// Having this set to None, disables this feature. Default value is 10. This is only
    /// applicable if the `enable_packet_filter` option is set.
    pub filter_max_nodes_per_ip: Option<usize>,

    /// The maximum number of nodes that can be banned by a single IP before that IP gets banned.
    /// The default is 5. This is only
    /// applicable if the `enable_packet_filter` option is set.
    pub filter_max_bans_per_ip: Option<usize>,

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
        // This is only applicable if enable_packet_filter is set.
        let filter_rate_limiter = Some(
            RateLimiterBuilder::new()
                .total_n_every(10, Duration::from_secs(1)) // Allow bursts, average 10 per second
                .node_n_every(8, Duration::from_secs(1)) // Allow bursts, average 8 per second
                .ip_n_every(9, Duration::from_secs(1)) // Allow bursts, average 9 per second
                .build()
                .expect("The total rate limit has been specified"),
        );

        Self {
            enable_packet_filter: false,
            request_timeout: Duration::from_secs(1),
            vote_duration: Duration::from_secs(30),
            query_peer_timeout: Duration::from_secs(2),
            query_timeout: Duration::from_secs(60),
            request_retries: 1,
            session_timeout: Duration::from_secs(86400),
            session_cache_capacity: 1000,
            ip_limit: false,
            incoming_bucket_limit: MAX_NODES_PER_BUCKET,
            ping_interval: Duration::from_secs(300),
            report_discovered_peers: true,
            filter_rate_limiter,
            filter_max_nodes_per_ip: Some(10),
            filter_max_bans_per_ip: Some(5),
            permit_ban_list: PermitBanList::default(),
            ban_duration: Some(Duration::from_secs(3600)), // 1 hour
            ip_mode: IpMode::default(),
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

    /// The interval over which votes are remembered when determining our external IP. A lower
    /// interval will respond faster to IP changes. Default is 30 seconds.
    pub fn vote_duration(&mut self, vote_duration: Duration) -> &mut Self {
        self.config.vote_duration = vote_duration;
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

    /// The maximum number of established sessions to maintain.
    pub fn session_cache_capacity(&mut self, capacity: usize) -> &mut Self {
        self.config.session_cache_capacity = capacity;
        self
    }

    /// Limits the number of IP addresses from the same
    /// /24 subnet in the kbuckets table. This is to mitigate eclipse attacks.
    pub fn ip_limit(&mut self) -> &mut Self {
        self.config.ip_limit = true;
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

    /// A rate limiter for limiting inbound requests.
    pub fn filter_rate_limiter(&mut self, rate_limiter: Option<RateLimiter>) -> &mut Self {
        self.config.filter_rate_limiter = rate_limiter;
        self
    }

    /// If the filter is enabled, sets the maximum number of nodes per IP before banning
    /// the IP.
    pub fn filter_max_nodes_per_ip(&mut self, max_nodes_per_ip: Option<usize>) -> &mut Self {
        self.config.filter_max_nodes_per_ip = max_nodes_per_ip;
        self
    }

    /// The maximum number of times nodes from a single IP can be banned, before the IP itself
    /// gets banned.
    pub fn filter_max_bans_per_ip(&mut self, max_bans_per_ip: Option<usize>) -> &mut Self {
        self.config.filter_max_bans_per_ip = max_bans_per_ip;
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

    /// Configures the type of socket to bind to. This also affects the selection of address to use
    /// to contact an ENR.
    pub fn ip_mode(&mut self, ip_mode: IpMode) -> &mut Self {
        self.config.ip_mode = ip_mode;
        self
    }

    pub fn build(&mut self) -> Discv4Config {
        assert!(self.config.incoming_bucket_limit <= MAX_NODES_PER_BUCKET);

        self.config.clone()
    }
}
