//! Wrapper around [`discv5::Discv5`].

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::{
    collections::HashSet,
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use ::enr::Enr;
use alloy_primitives::bytes::Bytes;
use discv5::ListenConfig;
use enr::{discv4_id_to_discv5_id, EnrCombinedKeyWrapper};
use futures::future::join_all;
use itertools::Itertools;
use rand::{Rng, RngCore};
use reth_ethereum_forks::{EnrForkIdEntry, ForkId};
use reth_network_peers::{NodeRecord, PeerId};
use secp256k1::SecretKey;
use tokio::{sync::mpsc, task};
use tracing::{debug, error, trace};

pub mod config;
pub mod enr;
pub mod error;
pub mod filter;
pub mod metrics;
pub mod network_stack_id;

pub use discv5::{self, IpMode};

pub use config::{
    BootNode, Config, ConfigBuilder, DEFAULT_COUNT_BOOTSTRAP_LOOKUPS, DEFAULT_DISCOVERY_V5_ADDR,
    DEFAULT_DISCOVERY_V5_ADDR_IPV6, DEFAULT_DISCOVERY_V5_PORT,
    DEFAULT_SECONDS_BOOTSTRAP_LOOKUP_INTERVAL, DEFAULT_SECONDS_LOOKUP_INTERVAL,
};
pub use enr::enr_to_discv4_id;
pub use error::Error;
pub use filter::{FilterOutcome, MustNotIncludeKeys};
pub use network_stack_id::NetworkStackId;

use metrics::{DiscoveredPeersMetrics, Discv5Metrics};

/// Max kbucket index is 255.
///
/// This is the max log2distance for 32 byte [`NodeId`](discv5::enr::NodeId) - 1. See <https://github.com/sigp/discv5/blob/e9e0d4f93ec35591832a9a8d937b4161127da87b/src/kbucket.rs#L586-L587>.
pub const MAX_KBUCKET_INDEX: usize = 255;

/// Default lowest kbucket index to attempt filling, in periodic look up query to populate kbuckets.
///
/// The peer at the 0th kbucket index is at log2distance 1 from the local node ID. See <https://github.com/sigp/discv5/blob/e9e0d4f93ec35591832a9a8d937b4161127da87b/src/kbucket.rs#L586-L587>.
///
/// Default is 0th index.
pub const DEFAULT_MIN_TARGET_KBUCKET_INDEX: usize = 0;

/// Transparent wrapper around [`discv5::Discv5`].
#[derive(Clone)]
pub struct Discv5 {
    /// sigp/discv5 node.
    discv5: Arc<discv5::Discv5>,
    /// [`IpMode`] of the `RLPx` network.
    rlpx_ip_mode: IpMode,
    /// Key used in kv-pair to ID chain, e.g. 'opstack' or 'eth'.
    fork_key: Option<&'static [u8]>,
    /// Filter applied to a discovered peers before passing it up to app.
    discovered_peer_filter: MustNotIncludeKeys,
    /// Metrics for underlying [`discv5::Discv5`] node and filtered discovered peers.
    metrics: Discv5Metrics,
}

impl Discv5 {
    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Minimal interface with `reth_network::discovery`
    ////////////////////////////////////////////////////////////////////////////////////////////////

    /// Adds the node to the table, if it is not already present.
    pub fn add_node(&self, node_record: Enr<SecretKey>) -> Result<(), Error> {
        let EnrCombinedKeyWrapper(enr) = node_record.into();
        self.discv5.add_enr(enr).map_err(Error::AddNodeFailed)
    }

    /// Sets the pair in the EIP-868 [`Enr`] of the node.
    ///
    /// If the key already exists, this will update it.
    ///
    /// CAUTION: The value **must** be rlp encoded
    pub fn set_eip868_in_local_enr(&self, key: Vec<u8>, rlp: Bytes) {
        let Ok(key_str) = std::str::from_utf8(&key) else {
            error!(target: "discv5",
                err="key not utf-8",
                "failed to update local enr"
            );
            return
        };
        if let Err(err) = self.discv5.enr_insert(key_str, &rlp) {
            error!(target: "discv5",
                %err,
                "failed to update local enr"
            );
        }
    }

    /// Sets the pair in the EIP-868 [`Enr`] of the node.
    ///
    /// If the key already exists, this will update it.
    pub fn encode_and_set_eip868_in_local_enr(
        &self,
        key: Vec<u8>,
        value: impl alloy_rlp::Encodable,
    ) {
        let mut buf = Vec::new();
        value.encode(&mut buf);
        self.set_eip868_in_local_enr(key, buf.into())
    }

    /// Adds the peer and id to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban(&self, peer_id: PeerId, ip: IpAddr) {
        match discv4_id_to_discv5_id(peer_id) {
            Ok(node_id) => {
                self.discv5.ban_node(&node_id, None);
                self.ban_ip(ip);
            }
            Err(err) => error!(target: "discv5",
                %err,
                "failed to ban peer"
            ),
        }
    }

    /// Adds the ip to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban_ip(&self, ip: IpAddr) {
        self.discv5.ban_ip(ip, None);
    }

    /// Returns the [`NodeRecord`] of the local node.
    ///
    /// This includes the currently tracked external IP address of the node.
    pub fn node_record(&self) -> NodeRecord {
        let enr: Enr<_> = EnrCombinedKeyWrapper(self.discv5.local_enr()).into();
        (&enr).try_into().unwrap()
    }

    /// Spawns [`discv5::Discv5`]. Returns [`discv5::Discv5`] handle in reth compatible wrapper type
    /// [`Discv5`], a receiver of [`discv5::Event`]s from the underlying node, and the local
    /// [`Enr`](discv5::Enr) converted into the reth compatible [`NodeRecord`] type.
    pub async fn start(
        sk: &SecretKey,
        discv5_config: Config,
    ) -> Result<(Self, mpsc::Receiver<discv5::Event>, NodeRecord), Error> {
        //
        // 1. make local enr from listen config
        //
        let (enr, bc_enr, fork_key, rlpx_ip_mode) = build_local_enr(sk, &discv5_config);

        trace!(target: "net::discv5",
            ?enr,
            "local ENR"
        );

        //
        // 2. start discv5
        //
        let Config {
            discv5_config,
            bootstrap_nodes,
            lookup_interval,
            bootstrap_lookup_interval,
            bootstrap_lookup_countdown,
            discovered_peer_filter,
            ..
        } = discv5_config;

        let EnrCombinedKeyWrapper(enr) = enr.into();
        let sk = discv5::enr::CombinedKey::secp256k1_from_bytes(&mut sk.secret_bytes()).unwrap();
        let mut discv5 = match discv5::Discv5::new(enr, sk, discv5_config) {
            Ok(discv5) => discv5,
            Err(err) => return Err(Error::InitFailure(err)),
        };
        discv5.start().await.map_err(Error::Discv5Error)?;

        // start discv5 updates stream
        let discv5_updates = discv5.event_stream().await.map_err(Error::Discv5Error)?;

        let discv5 = Arc::new(discv5);

        //
        // 3. add boot nodes
        //
        bootstrap(bootstrap_nodes, &discv5).await?;

        let metrics = Discv5Metrics::default();

        //
        // 4. start bg kbuckets maintenance
        //
        spawn_populate_kbuckets_bg(
            lookup_interval,
            bootstrap_lookup_interval,
            bootstrap_lookup_countdown,
            metrics.clone(),
            discv5.clone(),
        );

        Ok((
            Self { discv5, rlpx_ip_mode, fork_key, discovered_peer_filter, metrics },
            discv5_updates,
            bc_enr,
        ))
    }

    /// Process an event from the underlying [`discv5::Discv5`] node.
    pub fn on_discv5_update(&self, update: discv5::Event) -> Option<DiscoveredPeer> {
        #[allow(clippy::match_same_arms)]
        match update {
            discv5::Event::SocketUpdated(_) | discv5::Event::TalkRequest(_) |
            // `Discovered` not unique discovered peers
            discv5::Event::Discovered(_) => None,
            discv5::Event::NodeInserted { replaced: _, .. } => {

                // node has been inserted into kbuckets

                // `replaced` partly covers `reth_discv4::DiscoveryUpdate::Removed(_)`

                self.metrics.discovered_peers.increment_kbucket_insertions(1);

                None
            }
            discv5::Event::SessionEstablished(enr, remote_socket) => {
                // this branch is semantically similar to branches of
                // `reth_discv4::DiscoveryUpdate`: `DiscoveryUpdate::Added(_)` and
                // `DiscoveryUpdate::DiscoveredAtCapacity(_)

                // peer has been discovered as part of query, or, by incoming session (peer has
                // discovered us)

                self.metrics.discovered_peers.increment_established_sessions_raw(1);

                self.on_discovered_peer(&enr, remote_socket)
            }
            discv5::Event::UnverifiableEnr {
                enr,
                socket,
                node_id: _,
            } => {
                // this branch is semantically similar to branches of
                // `reth_discv4::DiscoveryUpdate`: `DiscoveryUpdate::Added(_)` and
                // `DiscoveryUpdate::DiscoveredAtCapacity(_)

                // peer has been discovered as part of query, or, by an outgoing session (but peer
                // is behind NAT and responds from a different socket)

                // NOTE: `discv5::Discv5` won't initiate a session with any peer with an
                // unverifiable node record, for example one that advertises a reserved LAN IP
                // address on a WAN network. This is in order to prevent DoS attacks, where some
                // malicious peers may advertise a victim's socket. We will still try and connect
                // to them over RLPx, to be compatible with EL discv5 implementations that don't
                // enforce this security measure.

                trace!(target: "net::discv5",
                    ?enr,
                    %socket,
                    "discovered unverifiable enr, source socket doesn't match socket advertised in ENR"
                );

                self.metrics.discovered_peers.increment_unverifiable_enrs_raw_total(1);

                self.on_discovered_peer(&enr, socket)
            }
            _ => None
        }
    }

    /// Processes a discovered peer. Returns `true` if peer is added to
    pub fn on_discovered_peer(
        &self,
        enr: &discv5::Enr,
        socket: SocketAddr,
    ) -> Option<DiscoveredPeer> {
        self.metrics.discovered_peers_advertised_networks.increment_once_by_network_type(enr);

        let node_record = match self.try_into_reachable(enr, socket) {
            Ok(enr_bc) => enr_bc,
            Err(err) => {
                trace!(target: "net::discv5",
                    %err,
                    ?enr,
                    "discovered peer is unreachable"
                );

                self.metrics.discovered_peers.increment_established_sessions_unreachable_enr(1);

                return None
            }
        };
        if let FilterOutcome::Ignore { reason } = self.filter_discovered_peer(enr) {
            trace!(target: "net::discv5",
                ?enr,
                reason,
                "filtered out discovered peer"
            );

            self.metrics.discovered_peers.increment_established_sessions_filtered(1);

            return None
        }

        // todo: extend for all network stacks in reth-network rlpx logic
        let fork_id = (self.fork_key == Some(NetworkStackId::ETH))
            .then(|| self.get_fork_id(enr).ok())
            .flatten();

        trace!(target: "net::discv5",
            ?fork_id,
            ?enr,
            "discovered peer"
        );

        Some(DiscoveredPeer { node_record, fork_id })
    }

    /// Tries to convert an [`Enr`](discv5::Enr) into the backwards compatible type [`NodeRecord`],
    /// w.r.t. local `RLPx` [`IpMode`]. Uses source socket as udp socket.
    pub fn try_into_reachable(
        &self,
        enr: &discv5::Enr,
        socket: SocketAddr,
    ) -> Result<NodeRecord, Error> {
        let id = enr_to_discv4_id(enr).ok_or(Error::IncompatibleKeyType)?;

        if enr.tcp4().is_none() && enr.tcp6().is_none() {
            return Err(Error::UnreachableRlpx)
        }
        let Some(tcp_port) = (match self.rlpx_ip_mode {
            IpMode::Ip4 => enr.tcp4(),
            IpMode::Ip6 => enr.tcp6(),
            _ => unimplemented!("dual-stack support not implemented for rlpx"),
        }) else {
            return Err(Error::IpVersionMismatchRlpx(self.rlpx_ip_mode))
        };

        Ok(NodeRecord { address: socket.ip(), tcp_port, udp_port: socket.port(), id })
    }

    /// Applies filtering rules on an ENR. Returns [`Ok`](FilterOutcome::Ok) if peer should be
    /// passed up to app, and [`Ignore`](FilterOutcome::Ignore) if peer should instead be dropped.
    pub fn filter_discovered_peer(&self, enr: &discv5::Enr) -> FilterOutcome {
        self.discovered_peer_filter.filter(enr)
    }

    /// Returns the [`ForkId`] of the given [`Enr`](discv5::Enr) w.r.t. the local node's network
    /// stack, if field is set.
    pub fn get_fork_id<K: discv5::enr::EnrKey>(
        &self,
        enr: &discv5::enr::Enr<K>,
    ) -> Result<ForkId, Error> {
        let Some(key) = self.fork_key else { return Err(Error::NetworkStackIdNotConfigured) };
        let fork_id = enr
            .get_decodable::<EnrForkIdEntry>(key)
            .ok_or(Error::ForkMissing(key))?
            .map(Into::into)?;

        Ok(fork_id)
    }

    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Interface with sigp/discv5
    ////////////////////////////////////////////////////////////////////////////////////////////////

    /// Exposes API of [`discv5::Discv5`].
    pub fn with_discv5<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&discv5::Discv5) -> R,
    {
        f(&self.discv5)
    }

    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Complementary
    ////////////////////////////////////////////////////////////////////////////////////////////////

    /// Returns the `RLPx` [`IpMode`] of the local node.
    pub const fn ip_mode(&self) -> IpMode {
        self.rlpx_ip_mode
    }

    /// Returns the key to use to identify the [`ForkId`] kv-pair on the [`Enr`](discv5::Enr).
    pub const fn fork_key(&self) -> Option<&[u8]> {
        self.fork_key
    }
}

impl fmt::Debug for Discv5 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "{ .. }".fmt(f)
    }
}

/// Result of successfully processing a peer discovered by [`discv5::Discv5`].
#[derive(Debug)]
pub struct DiscoveredPeer {
    /// A discovery v4 backwards compatible ENR.
    pub node_record: NodeRecord,
    /// [`ForkId`] extracted from ENR w.r.t. configured
    pub fork_id: Option<ForkId>,
}

/// Builds the local ENR with the supplied key.
pub fn build_local_enr(
    sk: &SecretKey,
    config: &Config,
) -> (Enr<SecretKey>, NodeRecord, Option<&'static [u8]>, IpMode) {
    let mut builder = discv5::enr::Enr::builder();

    let Config { discv5_config, fork, tcp_socket, other_enr_kv_pairs, .. } = config;

    let socket = match discv5_config.listen_config {
        ListenConfig::Ipv4 { ip, port } => {
            if ip != Ipv4Addr::UNSPECIFIED {
                builder.ip4(ip);
            }
            builder.udp4(port);
            builder.tcp4(tcp_socket.port());

            (ip, port).into()
        }
        ListenConfig::Ipv6 { ip, port } => {
            if ip != Ipv6Addr::UNSPECIFIED {
                builder.ip6(ip);
            }
            builder.udp6(port);
            builder.tcp6(tcp_socket.port());

            (ip, port).into()
        }
        ListenConfig::DualStack { ipv4, ipv4_port, ipv6, ipv6_port } => {
            if ipv4 != Ipv4Addr::UNSPECIFIED {
                builder.ip4(ipv4);
            }
            builder.udp4(ipv4_port);
            builder.tcp4(tcp_socket.port());

            if ipv6 != Ipv6Addr::UNSPECIFIED {
                builder.ip6(ipv6);
            }
            builder.udp6(ipv6_port);

            (ipv6, ipv6_port).into()
        }
    };

    let rlpx_ip_mode = if tcp_socket.is_ipv4() { IpMode::Ip4 } else { IpMode::Ip6 };

    // identifies which network node is on
    let network_stack_id = fork.as_ref().map(|(network_stack_id, fork_value)| {
        builder.add_value_rlp(network_stack_id, alloy_rlp::encode(fork_value).into());
        *network_stack_id
    });

    // add other data
    for (key, value) in other_enr_kv_pairs {
        builder.add_value_rlp(key, value.clone().into());
    }

    // enr v4 not to get confused with discv4, independent versioning enr and
    // discovery
    let enr = builder.build(sk).expect("should build enr v4");

    // backwards compatible enr
    let bc_enr = NodeRecord::from_secret_key(socket, sk);

    (enr, bc_enr, network_stack_id, rlpx_ip_mode)
}

/// Bootstraps underlying [`discv5::Discv5`] node with configured peers.
pub async fn bootstrap(
    bootstrap_nodes: HashSet<BootNode>,
    discv5: &Arc<discv5::Discv5>,
) -> Result<(), Error> {
    trace!(target: "net::discv5",
        ?bootstrap_nodes,
        "adding bootstrap nodes .."
    );

    let mut enr_requests = vec![];
    for node in bootstrap_nodes {
        match node {
            BootNode::Enr(node) => {
                if let Err(err) = discv5.add_enr(node) {
                    return Err(Error::AddNodeFailed(err))
                }
            }
            BootNode::Enode(enode) => {
                let discv5 = discv5.clone();
                enr_requests.push(async move {
                    if let Err(err) = discv5.request_enr(enode.to_string()).await {
                        debug!(target: "net::discv5",
                            ?enode,
                            %err,
                            "failed adding boot node"
                        );
                    }
                })
            }
        }
    }

    // If a session is established, the ENR is added straight away to discv5 kbuckets
    Ok(_ = join_all(enr_requests).await)
}

/// Backgrounds regular look up queries, in order to keep kbuckets populated.
pub fn spawn_populate_kbuckets_bg(
    lookup_interval: u64,
    bootstrap_lookup_interval: u64,
    bootstrap_lookup_countdown: u64,
    metrics: Discv5Metrics,
    discv5: Arc<discv5::Discv5>,
) {
    task::spawn({
        let local_node_id = discv5.local_enr().node_id();
        let lookup_interval = Duration::from_secs(lookup_interval);
        let metrics = metrics.discovered_peers;
        let mut kbucket_index = MAX_KBUCKET_INDEX;
        let pulse_lookup_interval = Duration::from_secs(bootstrap_lookup_interval);
        // todo: graceful shutdown

        async move {
            // make many fast lookup queries at bootstrap, trying to fill kbuckets at furthest
            // log2distance from local node
            for i in (0..bootstrap_lookup_countdown).rev() {
                let target = discv5::enr::NodeId::random();

                trace!(target: "net::discv5",
                    %target,
                    bootstrap_boost_runs_countdown=i,
                    lookup_interval=format!("{:#?}", pulse_lookup_interval),
                    "starting bootstrap boost lookup query"
                );

                lookup(target, &discv5, &metrics).await;

                tokio::time::sleep(pulse_lookup_interval).await;
            }

            // initiate regular lookups to populate kbuckets
            loop {
                // make sure node is connected to each subtree in the network by target
                // selection (ref kademlia)
                let target = get_lookup_target(kbucket_index, local_node_id);

                trace!(target: "net::discv5",
                    %target,
                    lookup_interval=format!("{:#?}", lookup_interval),
                    "starting periodic lookup query"
                );

                lookup(target, &discv5, &metrics).await;

                if kbucket_index > DEFAULT_MIN_TARGET_KBUCKET_INDEX {
                    // try to populate bucket one step closer
                    kbucket_index -= 1
                } else {
                    // start over with bucket furthest away
                    kbucket_index = MAX_KBUCKET_INDEX
                }

                tokio::time::sleep(lookup_interval).await;
            }
        }
    });
}

/// Gets the next lookup target, based on which bucket is currently being targeted.
pub fn get_lookup_target(
    kbucket_index: usize,
    local_node_id: discv5::enr::NodeId,
) -> discv5::enr::NodeId {
    // init target
    let mut target = local_node_id.raw();

    // make sure target has a 'log2distance'-long suffix that differs from local node id
    let bit_offset = MAX_KBUCKET_INDEX.saturating_sub(kbucket_index);
    let (byte, bit) = (bit_offset / 8, bit_offset % 8);
    // Flip the target bit.
    target[byte] ^= 1 << (7 - bit);

    // Randomize the bits after the target.
    let mut rng = rand::thread_rng();
    // Randomize remaining bits in the byte we modified.
    if bit < 7 {
        // Compute the mask of the bits that need to be randomized.
        let bits_to_randomize = 0xff >> (bit + 1);
        // Clear.
        target[byte] &= !bits_to_randomize;
        // Randomize.
        target[byte] |= rng.gen::<u8>() & bits_to_randomize;
    }
    // Randomize remaining bytes.
    rng.fill_bytes(&mut target[byte + 1..]);

    target.into()
}

/// Runs a [`discv5::Discv5`] lookup query.
pub async fn lookup(
    target: discv5::enr::NodeId,
    discv5: &discv5::Discv5,
    metrics: &DiscoveredPeersMetrics,
) {
    metrics.set_total_sessions(discv5.metrics().active_sessions);
    metrics.set_total_kbucket_peers(
        discv5.with_kbuckets(|kbuckets| kbuckets.read().iter_ref().count()),
    );

    match discv5.find_node(target).await {
        Err(err) => trace!(target: "net::discv5",
            %err,
            "lookup query failed"
        ),
        Ok(peers) => trace!(target: "net::discv5",
            target=format!("{:#?}", target),
            peers_count=peers.len(),
            peers=format!("[{:#}]", peers.iter()
                .map(|enr| enr.node_id()
            ).format(", ")),
            "peers returned by lookup query"
        ),
    }

    // `Discv5::connected_peers` can be subset of sessions, not all peers make it
    // into kbuckets, e.g. incoming sessions from peers with
    // unreachable enrs
    debug!(target: "net::discv5",
        connected_peers=discv5.connected_peers(),
        "connected peers in routing table"
    );
}

#[cfg(test)]
mod test {
    use super::*;
    use ::enr::{CombinedKey, EnrKey};
    use reth_chainspec::MAINNET;
    use secp256k1::rand::thread_rng;
    use tracing::trace;

    fn discv5_noop() -> Discv5 {
        let sk = CombinedKey::generate_secp256k1();
        Discv5 {
            discv5: Arc::new(
                discv5::Discv5::new(
                    Enr::empty(&sk).unwrap(),
                    sk,
                    discv5::ConfigBuilder::new(ListenConfig::default()).build(),
                )
                .unwrap(),
            ),
            rlpx_ip_mode: IpMode::Ip4,
            fork_key: None,
            discovered_peer_filter: MustNotIncludeKeys::default(),
            metrics: Discv5Metrics::default(),
        }
    }

    async fn start_discovery_node(
        udp_port_discv5: u16,
    ) -> (Discv5, mpsc::Receiver<discv5::Event>, NodeRecord) {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();
        let rlpx_addr: SocketAddr = "127.0.0.1:30303".parse().unwrap();

        let discv5_listen_config = ListenConfig::from(discv5_addr);
        let discv5_config = Config::builder(rlpx_addr)
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        Discv5::start(&secret_key, discv5_config).await.expect("should build discv5")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discv5() {
        reth_tracing::init_test_tracing();

        // rig test

        // rig node_1
        let (node_1, mut stream_1, _) = start_discovery_node(30344).await;
        let node_1_enr = node_1.with_discv5(|discv5| discv5.local_enr());

        // rig node_2
        let (node_2, mut stream_2, _) = start_discovery_node(30355).await;
        let node_2_enr = node_2.with_discv5(|discv5| discv5.local_enr());

        trace!(target: "net::discv5::test",
            node_1_node_id=format!("{:#}", node_1_enr.node_id()),
            node_2_node_id=format!("{:#}", node_2_enr.node_id()),
            "started nodes"
        );

        // test

        // add node_2 to discovery handle of node_1 (should add node to discv5 kbuckets)
        let node_2_enr_reth_compatible_ty: Enr<SecretKey> =
            EnrCombinedKeyWrapper(node_2_enr.clone()).into();
        node_1.add_node(node_2_enr_reth_compatible_ty).unwrap();

        // verify node_2 is in KBuckets of node_1:discv5
        assert!(
            node_1.with_discv5(|discv5| discv5.table_entries_id().contains(&node_2_enr.node_id()))
        );

        // manually trigger connection from node_1 to node_2
        node_1.with_discv5(|discv5| discv5.send_ping(node_2_enr.clone())).await.unwrap();

        // verify node_1:discv5 is connected to node_2:discv5 and vv
        let event_2_v5 = stream_2.recv().await.unwrap();
        let event_1_v5 = stream_1.recv().await.unwrap();
        assert!(matches!(
            event_1_v5,
            discv5::Event::SessionEstablished(node, socket) if node == node_2_enr && socket == node_2_enr.udp4_socket().unwrap().into()
        ));
        assert!(matches!(
            event_2_v5,
            discv5::Event::SessionEstablished(node, socket) if node == node_1_enr && socket == node_1_enr.udp4_socket().unwrap().into()
        ));

        // verify node_1 is in KBuckets of node_2:discv5
        let event_2_v5 = stream_2.recv().await.unwrap();
        assert!(matches!(
            event_2_v5,
            discv5::Event::NodeInserted { node_id, replaced } if node_id == node_1_enr.node_id() && replaced.is_none()
        ));
    }

    #[test]
    fn discovered_enr_disc_socket_missing() {
        reth_tracing::init_test_tracing();

        // rig test
        const REMOTE_RLPX_PORT: u16 = 30303;
        let remote_socket = "104.28.44.25:9000".parse().unwrap();
        let remote_key = CombinedKey::generate_secp256k1();
        let remote_enr = Enr::builder().tcp4(REMOTE_RLPX_PORT).build(&remote_key).unwrap();

        let discv5 = discv5_noop();

        // test
        let filtered_peer = discv5.on_discovered_peer(&remote_enr, remote_socket);

        assert_eq!(
            NodeRecord {
                address: remote_socket.ip(),
                udp_port: remote_socket.port(),
                tcp_port: REMOTE_RLPX_PORT,
                id: enr_to_discv4_id(&remote_enr).unwrap(),
            },
            filtered_peer.unwrap().node_record
        )
    }

    // Copied from sigp/discv5 with slight modification (U256 type)
    // <https://github.com/sigp/discv5/blob/master/src/kbucket/key.rs#L89-L101>
    #[allow(unreachable_pub)]
    #[allow(unused)]
    #[allow(clippy::assign_op_pattern)]
    mod sigp {
        use alloy_primitives::U256;
        use enr::{
            k256::sha2::digest::generic_array::{typenum::U32, GenericArray},
            NodeId,
        };

        /// A `Key` is a cryptographic hash, identifying both the nodes participating in
        /// the Kademlia DHT, as well as records stored in the DHT.
        ///
        /// The set of all `Key`s defines the Kademlia keyspace.
        ///
        /// `Key`s have an XOR metric as defined in the Kademlia paper, i.e. the bitwise XOR of
        /// the hash digests, interpreted as an integer. See [`Key::distance`].
        ///
        /// A `Key` preserves the preimage of type `T` of the hash function. See [`Key::preimage`].
        #[derive(Clone, Debug)]
        pub struct Key<T> {
            preimage: T,
            hash: GenericArray<u8, U32>,
        }

        impl<T> PartialEq for Key<T> {
            fn eq(&self, other: &Self) -> bool {
                self.hash == other.hash
            }
        }

        impl<T> Eq for Key<T> {}

        impl<TPeerId> AsRef<Self> for Key<TPeerId> {
            fn as_ref(&self) -> &Self {
                self
            }
        }

        impl<T> Key<T> {
            /// Construct a new `Key` by providing the raw 32 byte hash.
            pub const fn new_raw(preimage: T, hash: GenericArray<u8, U32>) -> Self {
                Self { preimage, hash }
            }

            /// Borrows the preimage of the key.
            pub const fn preimage(&self) -> &T {
                &self.preimage
            }

            /// Converts the key into its preimage.
            pub fn into_preimage(self) -> T {
                self.preimage
            }

            /// Computes the distance of the keys according to the XOR metric.
            pub fn distance<U>(&self, other: &Key<U>) -> Distance {
                let a = U256::from_be_slice(self.hash.as_slice());
                let b = U256::from_be_slice(other.hash.as_slice());
                Distance(a ^ b)
            }

            // Used in the FINDNODE query outside of the k-bucket implementation.
            /// Computes the integer log-2 distance between two keys, assuming a 256-bit
            /// key. The output returns None if the key's are identical. The range is 1-256.
            pub fn log2_distance<U>(&self, other: &Key<U>) -> Option<u64> {
                let xor_dist = self.distance(other);
                let log_dist = (256 - xor_dist.0.leading_zeros() as u64);
                (log_dist != 0).then_some(log_dist)
            }
        }

        impl From<NodeId> for Key<NodeId> {
            fn from(node_id: NodeId) -> Self {
                Self { preimage: node_id, hash: *GenericArray::from_slice(&node_id.raw()) }
            }
        }

        /// A distance between two `Key`s.
        #[derive(Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord, Debug)]
        pub struct Distance(pub(super) U256);
    }

    #[test]
    fn select_lookup_target() {
        for bucket_index in 0..=MAX_KBUCKET_INDEX {
            let sk = CombinedKey::generate_secp256k1();
            let local_node_id = discv5::enr::NodeId::from(sk.public());
            let target = get_lookup_target(bucket_index, local_node_id);

            let local_node_id = sigp::Key::from(local_node_id);
            let target = sigp::Key::from(target);

            assert_eq!(local_node_id.log2_distance(&target), Some(bucket_index as u64 + 1));
        }
    }

    #[test]
    fn build_enr_from_config() {
        const TCP_PORT: u16 = 30303;
        let fork_id = MAINNET.latest_fork_id();

        let config = Config::builder((Ipv4Addr::UNSPECIFIED, TCP_PORT).into())
            .fork(NetworkStackId::ETH, fork_id)
            .build();

        let sk = SecretKey::new(&mut thread_rng());
        let (enr, _, _, _) = build_local_enr(&sk, &config);

        let decoded_fork_id = enr
            .get_decodable::<EnrForkIdEntry>(NetworkStackId::ETH)
            .unwrap()
            .map(Into::into)
            .unwrap();

        assert_eq!(fork_id, decoded_fork_id);
        assert_eq!(TCP_PORT, enr.tcp4().unwrap()); // listen config is defaulting to ip mode ipv4
    }
}
