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
use alloy_rlp::Decodable;
use derive_more::Deref;
use discv5::ListenConfig;
use enr::{discv4_id_to_discv5_id, EnrCombinedKeyWrapper};
use futures::future::join_all;
use itertools::Itertools;
use reth_primitives::{bytes::Bytes, ForkId, NodeRecord, PeerId};
use secp256k1::SecretKey;
use tokio::{sync::mpsc, task};
use tracing::{debug, error, trace};

pub mod config;
pub mod enr;
pub mod error;
pub mod filter;
pub mod metrics;

pub use discv5::{self, IpMode};

pub use config::{BootNode, Config, ConfigBuilder};
pub use enr::enr_to_discv4_id;
pub use error::Error;
pub use filter::{FilterOutcome, MustNotIncludeKeys};
use metrics::Discv5Metrics;

/// The max log2 distance, is equivalent to the index of the last bit in a discv5 node id.
const MAX_LOG2_DISTANCE: usize = 255;

/// Transparent wrapper around [`discv5::Discv5`].
#[derive(Deref, Clone)]
pub struct Discv5 {
    #[deref]
    /// sigp/discv5 node.
    discv5: Arc<discv5::Discv5>,
    /// [`IpMode`] of the the node.
    ip_mode: IpMode,
    /// Key used in kv-pair to ID chain.
    fork_id_key: &'static [u8],
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
    pub fn add_node_to_routing_table(&self, node_record: Enr<SecretKey>) -> Result<(), Error> {
        let EnrCombinedKeyWrapper(enr) = node_record.into();
        self.add_enr(enr).map_err(Error::AddNodeToDiscv5Failed)
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
        if let Err(err) = self.enr_insert(key_str, &rlp) {
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
    pub fn ban_peer_by_ip_and_node_id(&self, peer_id: PeerId, ip: IpAddr) {
        match discv4_id_to_discv5_id(peer_id) {
            Ok(node_id) => {
                self.ban_node(&node_id, None);
                self.ban_peer_by_ip(ip);
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
    pub fn ban_peer_by_ip(&self, ip: IpAddr) {
        self.ban_ip(ip, None);
    }

    /// Returns the [`NodeRecord`] of the local node.
    ///
    /// This includes the currently tracked external IP address of the node.
    pub fn node_record(&self) -> NodeRecord {
        let enr: Enr<_> = EnrCombinedKeyWrapper(self.local_enr()).into();
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
        let Config {
            discv5_config,
            bootstrap_nodes,
            fork,
            tcp_port,
            other_enr_data,
            lookup_interval,
            discovered_peer_filter,
        } = discv5_config;

        let (enr, bc_enr, ip_mode, fork_id_key) = {
            let mut builder = discv5::enr::Enr::builder();

            let (ip_mode, socket) = match discv5_config.listen_config {
                ListenConfig::Ipv4 { ip, port } => {
                    if ip != Ipv4Addr::UNSPECIFIED {
                        builder.ip4(ip);
                    }
                    builder.udp4(port);
                    builder.tcp4(tcp_port);

                    (IpMode::Ip4, (ip, port).into())
                }
                ListenConfig::Ipv6 { ip, port } => {
                    if ip != Ipv6Addr::UNSPECIFIED {
                        builder.ip6(ip);
                    }
                    builder.udp6(port);
                    builder.tcp6(tcp_port);

                    (IpMode::Ip6, (ip, port).into())
                }
                ListenConfig::DualStack { ipv4, ipv4_port, ipv6, ipv6_port } => {
                    if ipv4 != Ipv4Addr::UNSPECIFIED {
                        builder.ip4(ipv4);
                    }
                    builder.udp4(ipv4_port);
                    builder.tcp4(tcp_port);

                    if ipv6 != Ipv6Addr::UNSPECIFIED {
                        builder.ip6(ipv6);
                    }
                    builder.udp6(ipv6_port);

                    (IpMode::DualStack, (ipv6, ipv6_port).into())
                }
            };

            // add fork id
            let (chain, fork_id) = fork;
            builder.add_value_rlp(chain, alloy_rlp::encode(fork_id).into());

            // add other data
            for (key, value) in other_enr_data {
                builder.add_value_rlp(key, alloy_rlp::encode(value).into());
            }

            // enr v4 not to get confused with discv4, independent versioning enr and
            // discovery
            let enr = builder.build(sk).expect("should build enr v4");
            let EnrCombinedKeyWrapper(enr) = enr.into();

            trace!(target: "net::discv5",
                ?enr,
                "local ENR"
            );

            // backwards compatible enr
            let bc_enr = NodeRecord::from_secret_key(socket, sk);

            (enr, bc_enr, ip_mode, chain)
        };

        //
        // 3. start discv5
        //
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
        // 4. add boot nodes
        //
        Self::bootstrap(bootstrap_nodes, &discv5)?;

        let metrics = Discv5Metrics::default();

        //
        // 5. bg kbuckets maintenance
        //
        Self::spawn_populate_kbuckets_bg(lookup_interval, metrics.clone(), discv5.clone());

        Ok((
            Self { discv5, ip_mode, fork_id_key, discovered_peer_filter, metrics },
            discv5_updates,
            bc_enr,
        ))
    }

    /// Bootstraps underlying [`discv5::Discv5`] node with configured peers.
    fn bootstrap(
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
                        return Err(Error::Discv5ErrorStr(err))
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
        _ = join_all(enr_requests);

        debug!(target: "net::discv5",
            nodes=format!("[{:#}]", discv5.with_kbuckets(|kbuckets| kbuckets
                .write()
                .iter()
                .map(|peer| format!("enr: {:?}, status: {:?}", peer.node.value, peer.status)).collect::<Vec<_>>()
            ).into_iter().format(", ")),
            "added boot nodes"
        );

        Ok(())
    }

    /// Backgrounds regular look up queries, in order to keep kbuckets populated.
    fn spawn_populate_kbuckets_bg(
        lookup_interval: u64,
        metrics: Discv5Metrics,
        discv5: Arc<discv5::Discv5>,
    ) {
        // initiate regular lookups to populate kbuckets
        task::spawn({
            let local_node_id = discv5.local_enr().node_id();
            let lookup_interval = Duration::from_secs(lookup_interval);
            let mut metrics = metrics.discovered_peers;
            let mut log2_distance = 0usize;
            // todo: graceful shutdown

            async move {
                loop {
                    metrics.set_total_sessions(discv5.metrics().active_sessions);
                    metrics.set_total_kbucket_peers(
                        discv5.with_kbuckets(|kbuckets| kbuckets.read().iter_ref().count()),
                    );

                    trace!(target: "net::discv5",
                        lookup_interval=format!("{:#?}", lookup_interval),
                        "starting periodic lookup query"
                    );
                    // make sure node is connected to each subtree in the network by target
                    // selection (ref kademlia)
                    let target = get_lookup_target(log2_distance, local_node_id);
                    if log2_distance < MAX_LOG2_DISTANCE {
                        // try to populate bucket one step further away
                        log2_distance += 1
                    } else {
                        // start over with self lookup
                        log2_distance = 0
                    }
                    match discv5.find_node(target).await {
                        Err(err) => trace!(target: "net::discv5",
                            lookup_interval=format!("{:#?}", lookup_interval),
                            %err,
                            "periodic lookup query failed"
                        ),
                        Ok(peers) => trace!(target: "net::discv5",
                            lookup_interval=format!("{:#?}", lookup_interval),
                            peers_count=peers.len(),
                            peers=format!("[{:#}]", peers.iter()
                                .map(|enr| enr.node_id()
                            ).format(", ")),
                            "peers returned by periodic lookup query"
                        ),
                    }

                    // `Discv5::connected_peers` can be subset of sessions, not all peers make it
                    // into kbuckets, e.g. incoming sessions from peers with
                    // unreachable enrs
                    debug!(target: "net::discv5",
                        connected_peers=discv5.connected_peers(),
                        "connected peers in routing table"
                    );
                    tokio::time::sleep(lookup_interval).await;
                }
            }
        });
    }

    /// Process an event from the underlying [`discv5::Discv5`] node.
    pub fn on_discv5_update(&mut self, update: discv5::Event) -> Option<DiscoveredPeer> {
        match update {
            discv5::Event::SocketUpdated(_) | discv5::Event::TalkRequest(_) |
            // `EnrAdded` not used in discv5 codebase
            discv5::Event::EnrAdded { .. } |
            // `Discovered` not unique discovered peers
            discv5::Event::Discovered(_) => None,
            discv5::Event::NodeInserted { replaced: _, .. } => {

                // node has been inserted into kbuckets

                // `replaced` partly covers `reth_discv4::DiscoveryUpdate::Removed(_)`

                self.metrics.discovered_peers.increment_kbucket_insertions(1);

                None
            }
            discv5::Event::SessionEstablished(enr, remote_socket) => {
                // covers `reth_discv4::DiscoveryUpdate` equivalents `DiscoveryUpdate::Added(_)` 
                // and `DiscoveryUpdate::DiscoveredAtCapacity(_)

                // peer has been discovered as part of query, or, by incoming session (peer has 
                // discovered us)

                self.metrics.discovered_peers_advertised_networks.increment_once_by_network_type(&enr);

                self.metrics.discovered_peers.increment_established_sessions_raw(1);

                self.on_discovered_peer(&enr, remote_socket)
            }
        }
    }

    /// Processes a discovered peer. Returns `true` if peer is added to
    fn on_discovered_peer(
        &mut self,
        enr: &discv5::Enr,
        socket: SocketAddr,
    ) -> Option<DiscoveredPeer> {
        let node_record = match self.try_into_reachable(enr, socket) {
            Ok(enr_bc) => enr_bc,
            Err(err) => {
                trace!(target: "net::discovery::discv5",
                    %err,
                    "discovered peer is unreachable"
                );

                self.metrics.discovered_peers.increment_established_sessions_unreachable_enr(1);

                return None
            }
        };
        let fork_id = match self.filter_discovered_peer(enr) {
            FilterOutcome::Ok => self.get_fork_id(enr).ok(),
            FilterOutcome::Ignore { reason } => {
                trace!(target: "net::discovery::discv5",
                    ?enr,
                    reason,
                    "filtered out discovered peer"
                );

                self.metrics.discovered_peers.increment_established_sessions_filtered(1);

                return None
            }
        };

        trace!(target: "net::discovery::discv5",
            ?fork_id,
            ?enr,
            "discovered peer"
        );

        Some(DiscoveredPeer { node_record, fork_id })
    }

    /// Tries to convert an [`Enr`](discv5::Enr) into the backwards compatible type [`NodeRecord`],
    /// w.r.t. local [`IpMode`]. Tries the socket from which the ENR was sent, if socket is missing
    /// from ENR.
    ///
    ///  Note: [`discv5::Discv5`] won't initiate a session with any peer with a malformed node
    /// record, that advertises a reserved IP address on a WAN network.
    fn try_into_reachable(
        &self,
        enr: &discv5::Enr,
        socket: SocketAddr,
    ) -> Result<NodeRecord, Error> {
        let id = enr_to_discv4_id(enr).ok_or(Error::IncompatibleKeyType)?;

        let udp_socket = self.ip_mode().get_contactable_addr(enr).unwrap_or(socket);

        // since we, on bootstrap, set tcp4 in local ENR for `IpMode::Dual`, we prefer tcp4 here
        // too
        let Some(tcp_port) = (match self.ip_mode() {
            IpMode::Ip4 | IpMode::DualStack => enr.tcp4(),
            IpMode::Ip6 => enr.tcp6(),
        }) else {
            return Err(Error::IpVersionMismatchRlpx(self.ip_mode()))
        };

        Ok(NodeRecord { address: udp_socket.ip(), tcp_port, udp_port: udp_socket.port(), id })
    }

    /// Applies filtering rules on an ENR. Returns [`Ok`](FilterOutcome::Ok) if peer should be
    /// passed up to app, and [`Ignore`](FilterOutcome::Ignore) if peer should instead be dropped.
    fn filter_discovered_peer(&self, enr: &discv5::Enr) -> FilterOutcome {
        self.discovered_peer_filter.filter(enr)
    }

    /// Returns the [`ForkId`] of the given [`Enr`](discv5::Enr), if field is set.
    fn get_fork_id<K: discv5::enr::EnrKey>(
        &self,
        enr: &discv5::enr::Enr<K>,
    ) -> Result<ForkId, Error> {
        let mut fork_id_bytes = enr.get_raw_rlp(self.fork_id_key()).ok_or(Error::ForkMissing)?;

        Ok(ForkId::decode(&mut fork_id_bytes)?)
    }

    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Interface with sigp/discv5
    ////////////////////////////////////////////////////////////////////////////////////////////////

    /// Exposes API of [`discv5::Discv5`].
    pub fn with_discv5<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Self) -> R,
    {
        f(self)
    }

    ////////////////////////////////////////////////////////////////////////////////////////////////
    // Complementary
    ////////////////////////////////////////////////////////////////////////////////////////////////

    /// Returns the [`IpMode`] of the local node.
    pub fn ip_mode(&self) -> IpMode {
        self.ip_mode
    }

    /// Returns the key to use to identify the [`ForkId`] kv-pair on the [`Enr`](discv5::Enr).
    pub fn fork_id_key(&self) -> &[u8] {
        self.fork_id_key
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

/// Gets the next lookup target, based on which distance is currently being targeted.
pub fn get_lookup_target(
    log2_distance: usize,
    local_node_id: discv5::enr::NodeId,
) -> discv5::enr::NodeId {
    let mut target = local_node_id.raw();
    //make sure target has a 'distance'-long suffix that differs from local node id
    if log2_distance != 0 {
        let suffix_bit_offset = MAX_LOG2_DISTANCE.saturating_sub(log2_distance);
        let suffix_byte_offset = suffix_bit_offset / 8;
        // todo: flip the precise bit
        // let rel_suffix_bit_offset = suffix_bit_offset % 8;
        target[suffix_byte_offset] = !target[suffix_byte_offset];

        if suffix_byte_offset != 31 {
            for b in target.iter_mut().take(31).skip(suffix_byte_offset + 1) {
                *b = rand::random::<u8>();
            }
        }
    }

    target.into()
}

#[cfg(test)]
mod tests {
    use ::enr::{CombinedKey, EnrKey};
    use rand::Rng;
    use secp256k1::rand::thread_rng;
    use tracing::trace;

    use super::*;

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
            ip_mode: IpMode::Ip4,
            fork_id_key: b"noop",
            discovered_peer_filter: MustNotIncludeKeys::default(),
            metrics: Discv5Metrics::default(),
        }
    }

    async fn start_discovery_node(
        udp_port_discv5: u16,
    ) -> (Discv5, mpsc::Receiver<discv5::Event>, NodeRecord) {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        let discv5_listen_config = ListenConfig::from(discv5_addr);
        let discv5_config = Config::builder(30303)
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

        trace!(target: "net::discovery::tests",
            node_1_node_id=format!("{:#}", node_1_enr.node_id()),
            node_2_node_id=format!("{:#}", node_2_enr.node_id()),
            "started nodes"
        );

        // test

        // add node_2 to discovery handle of node_1 (should add node to discv5 kbuckets)
        let node_2_enr_reth_compatible_ty: Enr<SecretKey> =
            EnrCombinedKeyWrapper(node_2_enr.clone()).into();
        node_1.add_node_to_routing_table(node_2_enr_reth_compatible_ty).unwrap();

        // verify node_2 is in KBuckets of node_1:discv5
        assert!(
            node_1.with_discv5(|discv5| discv5.table_entries_id().contains(&node_2_enr.node_id()))
        );

        // manually trigger connection from node_1 to node_2
        node_1.with_discv5(|discv5| discv5.send_ping(node_2_enr.clone())).await.unwrap();

        // verify node_1:discv5 is connected to node_2:discv5 and vv
        let event_2_v5 = stream_2.recv().await.unwrap();
        let event_1_v5 = stream_1.recv().await.unwrap();
        matches!(
            event_1_v5,
            discv5::Event::SessionEstablished(node, socket) if node == node_2_enr && socket == node_2_enr.udp4_socket().unwrap().into()
        );
        matches!(
            event_2_v5,
            discv5::Event::SessionEstablished(node, socket) if node == node_1_enr && socket == node_1_enr.udp4_socket().unwrap().into()
        );

        // verify node_1 is in KBuckets of node_2:discv5
        let event_2_v5 = stream_2.recv().await.unwrap();
        matches!(
            event_2_v5,
            discv5::Event::NodeInserted { node_id, replaced } if node_id == node_1_enr.node_id() && replaced.is_none()
        );
    }

    #[test]
    fn discovered_enr_disc_socket_missing() {
        reth_tracing::init_test_tracing();

        // rig test
        const REMOTE_RLPX_PORT: u16 = 30303;
        let remote_socket = "104.28.44.25:9000".parse().unwrap();
        let remote_key = CombinedKey::generate_secp256k1();
        let remote_enr = Enr::builder().tcp4(REMOTE_RLPX_PORT).build(&remote_key).unwrap();

        let mut discv5 = discv5_noop();

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
        use enr::{
            k256::sha2::digest::generic_array::{typenum::U32, GenericArray},
            NodeId,
        };
        use reth_primitives::U256;

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
            fn eq(&self, other: &Key<T>) -> bool {
                self.hash == other.hash
            }
        }

        impl<T> Eq for Key<T> {}

        impl<TPeerId> AsRef<Key<TPeerId>> for Key<TPeerId> {
            fn as_ref(&self) -> &Key<TPeerId> {
                self
            }
        }

        impl<T> Key<T> {
            /// Construct a new `Key` by providing the raw 32 byte hash.
            pub fn new_raw(preimage: T, hash: GenericArray<u8, U32>) -> Key<T> {
                Key { preimage, hash }
            }

            /// Borrows the preimage of the key.
            pub fn preimage(&self) -> &T {
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
                if log_dist == 0 {
                    None
                } else {
                    Some(log_dist)
                }
            }
        }

        impl From<NodeId> for Key<NodeId> {
            fn from(node_id: NodeId) -> Self {
                Key { preimage: node_id, hash: *GenericArray::from_slice(&node_id.raw()) }
            }
        }

        /// A distance between two `Key`s.
        #[derive(Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord, Debug)]
        pub struct Distance(pub(super) U256);
    }

    #[test]
    fn select_lookup_target() {
        // distance ceiled to the next byte
        const fn expected_log2_distance(log2_distance: usize) -> u64 {
            let log2_distance = log2_distance / 8;
            ((log2_distance + 1) * 8) as u64
        }

        let log2_distance = rand::thread_rng().gen_range(0..=MAX_LOG2_DISTANCE);

        let sk = CombinedKey::generate_secp256k1();
        let local_node_id = discv5::enr::NodeId::from(sk.public());
        let target = get_lookup_target(log2_distance, local_node_id);

        let local_node_id = sigp::Key::from(local_node_id);
        let target = sigp::Key::from(target);

        assert_eq!(
            expected_log2_distance(log2_distance),
            local_node_id.log2_distance(&target).unwrap()
        );
    }
}
