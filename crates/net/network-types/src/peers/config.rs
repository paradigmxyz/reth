//! Configuration for peering.

use std::{
    collections::HashSet,
    io::{self, ErrorKind},
    path::Path,
    time::Duration,
};

use reth_net_banlist::BanList;
use reth_network_peers::{NodeRecord, TrustedPeer};
use tracing::info;

use crate::{BackoffKind, ReputationChangeWeights};

/// Maximum number of available slots for outbound sessions.
pub const DEFAULT_MAX_COUNT_PEERS_OUTBOUND: u32 = 100;

/// Maximum number of available slots for inbound sessions.
pub const DEFAULT_MAX_COUNT_PEERS_INBOUND: u32 = 30;

/// Maximum number of available slots for concurrent outgoing dials.
///
/// This restricts how many outbound dials can be performed concurrently.
pub const DEFAULT_MAX_COUNT_CONCURRENT_OUTBOUND_DIALS: usize = 15;

/// The durations to use when a backoff should be applied to a peer.
///
/// See also [`BackoffKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PeerBackoffDurations {
    /// Applies to connection problems where there is a chance that they will be resolved after the
    /// short duration.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub low: Duration,
    /// Applies to more severe connection problems where there is a lower chance that they will be
    /// resolved.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub medium: Duration,
    /// Intended for spammers, or bad peers in general.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub high: Duration,
    /// Maximum total backoff duration.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub max: Duration,
}

impl PeerBackoffDurations {
    /// Returns the corresponding [`Duration`]
    pub const fn backoff(&self, kind: BackoffKind) -> Duration {
        match kind {
            BackoffKind::Low => self.low,
            BackoffKind::Medium => self.medium,
            BackoffKind::High => self.high,
        }
    }

    /// Returns the timestamp until which we should backoff.
    ///
    /// The Backoff duration is capped by the configured maximum backoff duration.
    pub fn backoff_until(&self, kind: BackoffKind, backoff_counter: u8) -> std::time::Instant {
        let backoff_time = self.backoff(kind);
        let backoff_time = backoff_time + backoff_time * backoff_counter as u32;
        let now = std::time::Instant::now();
        now + backoff_time.min(self.max)
    }

    /// Returns durations for testing.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn test() -> Self {
        Self {
            low: Duration::from_millis(200),
            medium: Duration::from_millis(200),
            high: Duration::from_millis(200),
            max: Duration::from_millis(200),
        }
    }
}

impl Default for PeerBackoffDurations {
    fn default() -> Self {
        Self {
            low: Duration::from_secs(30),
            // 3min
            medium: Duration::from_secs(60 * 3),
            // 15min
            high: Duration::from_secs(60 * 15),
            // 1h
            max: Duration::from_secs(60 * 60),
        }
    }
}

/// Tracks stats about connected nodes
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct ConnectionsConfig {
    /// Maximum allowed outbound connections.
    pub max_outbound: usize,
    /// Maximum allowed inbound connections.
    pub max_inbound: usize,
    /// Maximum allowed concurrent outbound dials.
    #[cfg_attr(feature = "serde", serde(default))]
    pub max_concurrent_outbound_dials: usize,
}

impl Default for ConnectionsConfig {
    fn default() -> Self {
        Self {
            max_outbound: DEFAULT_MAX_COUNT_PEERS_OUTBOUND as usize,
            max_inbound: DEFAULT_MAX_COUNT_PEERS_INBOUND as usize,
            max_concurrent_outbound_dials: DEFAULT_MAX_COUNT_CONCURRENT_OUTBOUND_DIALS,
        }
    }
}

/// Config type for initiating a `PeersManager` instance.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct PeersConfig {
    /// How often to recheck free slots for outbound connections.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub refill_slots_interval: Duration,
    /// Trusted nodes to connect to or accept from
    pub trusted_nodes: Vec<TrustedPeer>,
    /// Connect to or accept from trusted nodes only?
    #[cfg_attr(feature = "serde", serde(alias = "connect_trusted_nodes_only"))]
    pub trusted_nodes_only: bool,
    /// Maximum number of backoff attempts before we give up on a peer and dropping.
    ///
    /// The max time spent of a peer before it's removed from the set is determined by the
    /// configured backoff duration and the max backoff count.
    ///
    /// With a backoff counter of 5 and a backoff duration of 1h, the minimum time spent of the
    /// peer in the table is the sum of all backoffs (1h + 2h + 3h + 4h + 5h = 15h).
    ///
    /// Note: this does not apply to trusted peers.
    pub max_backoff_count: u8,
    /// Basic nodes to connect to.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub basic_nodes: HashSet<NodeRecord>,
    /// How long to ban bad peers.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub ban_duration: Duration,
    /// Restrictions on `PeerIds` and Ips.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub ban_list: BanList,
    /// Restrictions on connections.
    pub connection_info: ConnectionsConfig,
    /// How to weigh reputation changes.
    pub reputation_weights: ReputationChangeWeights,
    /// How long to backoff peers that we are failed to connect to for non-fatal reasons.
    ///
    /// The backoff duration increases with number of backoff attempts.
    pub backoff_durations: PeerBackoffDurations,
}

impl Default for PeersConfig {
    fn default() -> Self {
        Self {
            refill_slots_interval: Duration::from_millis(5_000),
            connection_info: Default::default(),
            reputation_weights: Default::default(),
            ban_list: Default::default(),
            // Ban peers for 12h
            ban_duration: Duration::from_secs(60 * 60 * 12),
            backoff_durations: Default::default(),
            trusted_nodes: Default::default(),
            trusted_nodes_only: false,
            basic_nodes: Default::default(),
            max_backoff_count: 5,
        }
    }
}

impl PeersConfig {
    /// A set of `peer_ids` and ip addr that we want to never connect to
    pub fn with_ban_list(mut self, ban_list: BanList) -> Self {
        self.ban_list = ban_list;
        self
    }

    /// Configure how long to ban bad peers
    pub const fn with_ban_duration(mut self, ban_duration: Duration) -> Self {
        self.ban_duration = ban_duration;
        self
    }

    /// Maximum allowed outbound connections.
    pub const fn with_max_outbound(mut self, max_outbound: usize) -> Self {
        self.connection_info.max_outbound = max_outbound;
        self
    }

    /// Maximum allowed inbound connections with optional update.
    pub const fn with_max_inbound_opt(mut self, max_inbound: Option<usize>) -> Self {
        if let Some(max_inbound) = max_inbound {
            self.connection_info.max_inbound = max_inbound;
        }
        self
    }

    /// Maximum allowed outbound connections with optional update.
    pub const fn with_max_outbound_opt(mut self, max_outbound: Option<usize>) -> Self {
        if let Some(max_outbound) = max_outbound {
            self.connection_info.max_outbound = max_outbound;
        }
        self
    }

    /// Maximum allowed inbound connections.
    pub const fn with_max_inbound(mut self, max_inbound: usize) -> Self {
        self.connection_info.max_inbound = max_inbound;
        self
    }

    /// Maximum allowed concurrent outbound dials.
    pub const fn with_max_concurrent_dials(mut self, max_concurrent_outbound_dials: usize) -> Self {
        self.connection_info.max_concurrent_outbound_dials = max_concurrent_outbound_dials;
        self
    }

    /// Nodes to always connect to.
    pub fn with_trusted_nodes(mut self, nodes: Vec<TrustedPeer>) -> Self {
        self.trusted_nodes = nodes;
        self
    }

    /// Connect only to trusted nodes.
    pub const fn with_trusted_nodes_only(mut self, trusted_only: bool) -> Self {
        self.trusted_nodes_only = trusted_only;
        self
    }

    /// Nodes available at launch.
    pub fn with_basic_nodes(mut self, nodes: HashSet<NodeRecord>) -> Self {
        self.basic_nodes = nodes;
        self
    }

    /// Configures the max allowed backoff count.
    pub const fn with_max_backoff_count(mut self, max_backoff_count: u8) -> Self {
        self.max_backoff_count = max_backoff_count;
        self
    }

    /// Configures how to weigh reputation changes.
    pub const fn with_reputation_weights(
        mut self,
        reputation_weights: ReputationChangeWeights,
    ) -> Self {
        self.reputation_weights = reputation_weights;
        self
    }

    /// Configures how long to backoff peers that are we failed to connect to for non-fatal reasons
    pub const fn with_backoff_durations(mut self, backoff_durations: PeerBackoffDurations) -> Self {
        self.backoff_durations = backoff_durations;
        self
    }

    /// Returns the maximum number of peers, inbound and outbound.
    pub const fn max_peers(&self) -> usize {
        self.connection_info.max_outbound + self.connection_info.max_inbound
    }

    /// Read from file nodes available at launch. Ignored if None.
    pub fn with_basic_nodes_from_file(
        self,
        optional_file: Option<impl AsRef<Path>>,
    ) -> Result<Self, io::Error> {
        let Some(file_path) = optional_file else { return Ok(self) };
        let reader = match std::fs::File::open(file_path.as_ref()) {
            Ok(file) => io::BufReader::new(file),
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(self),
            Err(e) => Err(e)?,
        };
        info!(target: "net::peers", file = %file_path.as_ref().display(), "Loading saved peers");
        let nodes: HashSet<NodeRecord> = serde_json::from_reader(reader)?;
        Ok(self.with_basic_nodes(nodes))
    }

    /// Returns settings for testing
    #[cfg(any(test, feature = "test-utils"))]
    pub fn test() -> Self {
        Self {
            refill_slots_interval: Duration::from_millis(100),
            backoff_durations: PeerBackoffDurations::test(),
            ban_duration: Duration::from_millis(200),
            ..Default::default()
        }
    }
}
