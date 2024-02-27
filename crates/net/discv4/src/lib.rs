//! Discovery v4 implementation: <https://github.com/ethereum/devp2p/blob/master/discv4.md>
//!
//! Discv4 employs a kademlia-like routing table to store and manage discovered peers and topics.
//! The protocol allows for external IP discovery in NAT environments through regular PING/PONG's
//! with discovered nodes. Nodes return the external IP address that they have received and a simple
//! majority is chosen as our external IP address. If an external IP address is updated, this is
//! produced as an event to notify the swarm (if one is used for this behaviour).
//!
//! This implementation consists of a [`Discv4`] and [`Discv4Service`] pair. The service manages the
//! state and drives the UDP socket. The (optional) [`Discv4`] serves as the frontend to interact
//! with the service via a channel. Whenever the underlying table changes service produces a
//! [`DiscoveryUpdate`] that listeners will receive.
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use crate::{
    error::{DecodePacketError, Discv4Error},
    proto::{FindNode, Message, Neighbours, Packet, Ping, Pong},
};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use discv5::{
    kbucket,
    kbucket::{
        BucketInsertResult, Distance, Entry as BucketEntry, InsertResult, KBucketsTable,
        NodeStatus, MAX_NODES_PER_BUCKET,
    },
    ConnectionDirection, ConnectionState,
};
use enr::{Enr, EnrBuilder};
use parking_lot::Mutex;
use proto::{EnrRequest, EnrResponse, EnrWrapper};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    hex, ForkId, PeerId, B256,
};
use secp256k1::SecretKey;
use std::{
    cell::RefCell,
    collections::{btree_map, hash_map::Entry, BTreeMap, HashMap, VecDeque},
    fmt, io,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    pin::Pin,
    rc::Rc,
    sync::Arc,
    task::{ready, Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, mpsc::error::TrySendError, oneshot, oneshot::Sender as OneshotSender},
    task::{JoinHandle, JoinSet},
    time::Interval,
};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tracing::{debug, trace};

pub mod error;
pub mod proto;

mod config;
pub use config::{Discv4Config, Discv4ConfigBuilder};

mod node;
use node::{kad_key, NodeKey};

mod table;

// reexport NodeRecord primitive
pub use reth_primitives::NodeRecord;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

use crate::table::PongTable;
use reth_net_nat::ResolveNatInterval;
/// reexport to get public ip.
pub use reth_net_nat::{external_ip, NatResolver};

/// The default address for discv4 via UDP
///
/// Note: the default TCP address is the same.
pub const DEFAULT_DISCOVERY_ADDR: Ipv4Addr = Ipv4Addr::UNSPECIFIED;

/// The default port for discv4 via UDP
///
/// Note: the default TCP port is the same.
pub const DEFAULT_DISCOVERY_PORT: u16 = 30303;

/// The default address for discv4 via UDP: "0.0.0.0:30303"
///
/// Note: The default TCP address is the same.
pub const DEFAULT_DISCOVERY_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT));

/// The maximum size of any packet is 1280 bytes.
const MAX_PACKET_SIZE: usize = 1280;

/// Length of the UDP datagram packet-header: Hash(32b) + Signature(65b) + Packet Type(1b)
const MIN_PACKET_SIZE: usize = 32 + 65 + 1;

/// Concurrency factor for `FindNode` requests to pick `ALPHA` closest nodes, <https://github.com/ethereum/devp2p/blob/master/discv4.md#recursive-lookup>
const ALPHA: usize = 3;

/// Maximum number of nodes to ping at concurrently. 2 full `Neighbours` responses with 16 _new_
/// nodes. This will apply some backpressure in recursive lookups.
const MAX_NODES_PING: usize = 2 * MAX_NODES_PER_BUCKET;

/// The size of the datagram is limited [`MAX_PACKET_SIZE`], 16 nodes, as the discv4 specifies don't
/// fit in one datagram. The safe number of nodes that always fit in a datagram is 12, with worst
/// case all of them being IPv6 nodes. This is calculated by `(MAX_PACKET_SIZE - (header + expire +
/// rlp overhead) / size(rlp(Node_IPv6))`
/// Even in the best case where all nodes are IPv4, only 14 nodes fit into one packet.
const SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS: usize = (MAX_PACKET_SIZE - 109) / 91;

/// The timeout used to identify expired nodes, 24h
///
/// Mirrors geth's `bondExpiration` of 24h
const ENDPOINT_PROOF_EXPIRATION: Duration = Duration::from_secs(24 * 60 * 60);

/// Duration used to expire nodes from the routing table 1hr
const EXPIRE_DURATION: Duration = Duration::from_secs(60 * 60);

// Restricts how many udp messages can be processed in a single [Discv4Service::poll] call.
//
// This will act as a manual yield point when draining the socket messages where the most CPU
// expensive part is handling outgoing messages: encoding and hashing the packet
const UDP_MESSAGE_POLL_LOOP_BUDGET: i32 = 4;

type EgressSender = mpsc::Sender<(Bytes, SocketAddr)>;
type EgressReceiver = mpsc::Receiver<(Bytes, SocketAddr)>;

pub(crate) type IngressSender = mpsc::Sender<IngressEvent>;
pub(crate) type IngressReceiver = mpsc::Receiver<IngressEvent>;

type NodeRecordSender = OneshotSender<Vec<NodeRecord>>;

/// The Discv4 frontend
///
/// This communicates with the [Discv4Service] by sending commands over a channel.
///
/// See also [Discv4::spawn]
#[derive(Debug, Clone)]
pub struct Discv4 {
    /// The address of the udp socket
    local_addr: SocketAddr,
    /// channel to send commands over to the service
    to_service: mpsc::UnboundedSender<Discv4Command>,
    /// Tracks the local node record.
    ///
    /// This includes the currently tracked external IP address of the node.
    node_record: Arc<Mutex<NodeRecord>>,
}

// === impl Discv4 ===

impl Discv4 {
    /// Same as [`Self::bind`] but also spawns the service onto a new task,
    /// [`Discv4Service::spawn()`]
    pub async fn spawn(
        local_address: SocketAddr,
        local_enr: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
    ) -> io::Result<Self> {
        let (discv4, service) = Self::bind(local_address, local_enr, secret_key, config).await?;

        service.spawn();

        Ok(discv4)
    }

    /// Returns a new instance with the given channel directly
    ///
    /// NOTE: this is only intended for test setups.
    #[cfg(feature = "test-utils")]
    pub fn noop() -> Self {
        let (to_service, _rx) = mpsc::unbounded_channel();
        let local_addr =
            (IpAddr::from(std::net::Ipv4Addr::UNSPECIFIED), DEFAULT_DISCOVERY_PORT).into();
        Self {
            local_addr,
            to_service,
            node_record: Arc::new(Mutex::new(NodeRecord::new(
                "127.0.0.1:3030".parse().unwrap(),
                PeerId::random(),
            ))),
        }
    }

    /// Binds a new UdpSocket and creates the service
    ///
    /// ```
    /// # use std::io;
    /// use rand::thread_rng;
    /// use reth_discv4::{Discv4, Discv4Config};
    /// use reth_primitives::{NodeRecord, PeerId};
    /// use secp256k1::SECP256K1;
    /// use std::{net::SocketAddr, str::FromStr};
    /// # async fn t() -> io::Result<()> {
    /// // generate a (random) keypair
    /// let mut rng = thread_rng();
    /// let (secret_key, pk) = SECP256K1.generate_keypair(&mut rng);
    /// let id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
    ///
    /// let socket = SocketAddr::from_str("0.0.0.0:0").unwrap();
    /// let local_enr =
    ///     NodeRecord { address: socket.ip(), tcp_port: socket.port(), udp_port: socket.port(), id };
    /// let config = Discv4Config::default();
    ///
    /// let (discv4, mut service) = Discv4::bind(socket, local_enr, secret_key, config).await.unwrap();
    ///
    /// // get an update strea
    /// let updates = service.update_stream();
    ///
    /// let _handle = service.spawn();
    ///
    /// // lookup the local node in the DHT
    /// let _discovered = discv4.lookup_self().await.unwrap();
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bind(
        local_address: SocketAddr,
        mut local_node_record: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
    ) -> io::Result<(Self, Discv4Service)> {
        let socket = UdpSocket::bind(local_address).await?;
        let local_addr = socket.local_addr()?;
        local_node_record.udp_port = local_addr.port();
        trace!(target: "discv4", ?local_addr,"opened UDP socket");

        let service = Discv4Service::new(socket, local_addr, local_node_record, secret_key, config);
        let discv4 = service.handle();
        Ok((discv4, service))
    }

    /// Returns the address of the UDP socket.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns the [NodeRecord] of the local node.
    ///
    /// This includes the currently tracked external IP address of the node.
    pub fn node_record(&self) -> NodeRecord {
        *self.node_record.lock()
    }

    /// Returns the currently tracked external IP of the node.
    pub fn external_ip(&self) -> IpAddr {
        self.node_record.lock().address
    }

    /// Sets the [Interval] used for periodically looking up targets over the network
    pub fn set_lookup_interval(&self, duration: Duration) {
        self.send_to_service(Discv4Command::SetLookupInterval(duration))
    }

    /// Starts a `FindNode` recursive lookup that locates the closest nodes to the given node id. See also: <https://github.com/ethereum/devp2p/blob/master/discv4.md#recursive-lookup>
    ///
    /// The lookup initiator starts by picking α closest nodes to the target it knows of. The
    /// initiator then sends concurrent FindNode packets to those nodes. α is a system-wide
    /// concurrency parameter, such as 3. In the recursive step, the initiator resends FindNode to
    /// nodes it has learned about from previous queries. Of the k nodes the initiator has heard of
    /// closest to the target, it picks α that it has not yet queried and resends FindNode to them.
    /// Nodes that fail to respond quickly are removed from consideration until and unless they do
    /// respond.
    //
    // If a round of FindNode queries fails to return a node any closer than the closest already
    // seen, the initiator resends the find node to all of the k closest nodes it has not already
    // queried. The lookup terminates when the initiator has queried and gotten responses from the k
    // closest nodes it has seen.
    pub async fn lookup_self(&self) -> Result<Vec<NodeRecord>, Discv4Error> {
        self.lookup_node(None).await
    }

    /// Looks up the given node id.
    ///
    /// Returning the closest nodes to the given node id.
    pub async fn lookup(&self, node_id: PeerId) -> Result<Vec<NodeRecord>, Discv4Error> {
        self.lookup_node(Some(node_id)).await
    }

    /// Performs a random lookup for node records.
    pub async fn lookup_random(&self) -> Result<Vec<NodeRecord>, Discv4Error> {
        let target = PeerId::random();
        self.lookup_node(Some(target)).await
    }

    /// Sends a message to the service to lookup the closest nodes
    pub fn send_lookup(&self, node_id: PeerId) {
        let cmd = Discv4Command::Lookup { node_id: Some(node_id), tx: None };
        self.send_to_service(cmd);
    }

    async fn lookup_node(&self, node_id: Option<PeerId>) -> Result<Vec<NodeRecord>, Discv4Error> {
        let (tx, rx) = oneshot::channel();
        let cmd = Discv4Command::Lookup { node_id, tx: Some(tx) };
        self.to_service.send(cmd)?;
        Ok(rx.await?)
    }

    /// Triggers a new self lookup without expecting a response
    pub fn send_lookup_self(&self) {
        let cmd = Discv4Command::Lookup { node_id: None, tx: None };
        self.send_to_service(cmd);
    }

    /// Removes the peer from the table, if it exists.
    pub fn remove_peer(&self, node_id: PeerId) {
        let cmd = Discv4Command::Remove(node_id);
        self.send_to_service(cmd);
    }

    /// Adds the node to the table, if it is not already present.
    pub fn add_node(&self, node_record: NodeRecord) {
        let cmd = Discv4Command::Add(node_record);
        self.send_to_service(cmd);
    }

    /// Adds the peer and id to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban(&self, node_id: PeerId, ip: IpAddr) {
        let cmd = Discv4Command::Ban(node_id, ip);
        self.send_to_service(cmd);
    }

    /// Adds the ip to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban_ip(&self, ip: IpAddr) {
        let cmd = Discv4Command::BanIp(ip);
        self.send_to_service(cmd);
    }

    /// Adds the peer to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban_node(&self, node_id: PeerId) {
        let cmd = Discv4Command::BanPeer(node_id);
        self.send_to_service(cmd);
    }

    /// Sets the tcp port
    ///
    /// This will update our [`NodeRecord`]'s tcp port.
    pub fn set_tcp_port(&self, port: u16) {
        let cmd = Discv4Command::SetTcpPort(port);
        self.send_to_service(cmd);
    }

    /// Sets the pair in the EIP-868 [`Enr`] of the node.
    ///
    /// If the key already exists, this will update it.
    ///
    /// CAUTION: The value **must** be rlp encoded
    pub fn set_eip868_rlp_pair(&self, key: Vec<u8>, rlp: Bytes) {
        let cmd = Discv4Command::SetEIP868RLPPair { key, rlp };
        self.send_to_service(cmd);
    }

    /// Sets the pair in the EIP-868 [`Enr`] of the node.
    ///
    /// If the key already exists, this will update it.
    pub fn set_eip868_rlp(&self, key: Vec<u8>, value: impl alloy_rlp::Encodable) {
        let mut buf = BytesMut::new();
        value.encode(&mut buf);
        self.set_eip868_rlp_pair(key, buf.freeze())
    }

    #[inline]
    fn send_to_service(&self, cmd: Discv4Command) {
        let _ = self.to_service.send(cmd).map_err(|err| {
            debug!(
                target: "discv4",
                %err,
                "channel capacity reached, dropping command",
            )
        });
    }

    /// Returns the receiver half of new listener channel that streams [`DiscoveryUpdate`]s.
    pub async fn update_stream(&self) -> Result<ReceiverStream<DiscoveryUpdate>, Discv4Error> {
        let (tx, rx) = oneshot::channel();
        let cmd = Discv4Command::Updates(tx);
        self.to_service.send(cmd)?;
        Ok(rx.await?)
    }

    /// Terminates the spawned [Discv4Service].
    pub fn terminate(&self) {
        self.send_to_service(Discv4Command::Terminated);
    }
}

/// Manages discv4 peer discovery over UDP.
///
/// This is a [Stream] to handles incoming and outgoing discv4 messages and emits updates via:
/// [Discv4Service::update_stream].
#[must_use = "Stream does nothing unless polled"]
pub struct Discv4Service {
    /// Local address of the UDP socket.
    local_address: SocketAddr,
    /// The local ENR for EIP-868 <https://eips.ethereum.org/EIPS/eip-868>
    local_eip_868_enr: Enr<SecretKey>,
    /// Local ENR of the server.
    local_node_record: NodeRecord,
    /// Keeps track of the node record of the local node.
    shared_node_record: Arc<Mutex<NodeRecord>>,
    /// The secret key used to sign payloads
    secret_key: SecretKey,
    /// The UDP socket for sending and receiving messages.
    _socket: Arc<UdpSocket>,
    /// The spawned UDP tasks.
    ///
    /// Note: If dropped, the spawned send+receive tasks are aborted.
    _tasks: JoinSet<()>,
    /// The routing table.
    kbuckets: KBucketsTable<NodeKey, NodeEntry>,
    /// Receiver for incoming messages
    ///
    /// Receives incoming messages from the UDP task.
    ingress: IngressReceiver,
    /// Sender for sending outgoing messages
    ///
    /// Sends outgoind messages to the UDP task.
    egress: EgressSender,
    /// Buffered pending pings to apply backpressure.
    ///
    /// Lookups behave like bursts of requests: Endpoint proof followed by `FindNode` request. [Recursive lookups](https://github.com/ethereum/devp2p/blob/master/discv4.md#recursive-lookup) can trigger multiple followup Pings+FindNode requests.
    /// A cap on concurrent `Ping` prevents escalation where: A large number of new nodes
    /// discovered via `FindNode` in a recursive lookup triggers a large number of `Ping`s, and
    /// followup `FindNode` requests.... Buffering them effectively prevents high `Ping` peaks.
    queued_pings: VecDeque<(NodeRecord, PingReason)>,
    /// Currently active pings to specific nodes.
    pending_pings: HashMap<PeerId, PingRequest>,
    /// Currently active endpoint proof verification lookups to specific nodes.
    ///
    /// Entries here means we've proven the peer's endpoint but haven't completed our end of the
    /// endpoint proof
    pending_lookup: HashMap<PeerId, (Instant, LookupContext)>,
    /// Currently active FindNode requests
    pending_find_nodes: HashMap<PeerId, FindNodeRequest>,
    /// Currently active ENR requests
    pending_enr_requests: HashMap<PeerId, EnrRequestState>,
    /// Copy of he sender half of the commands channel for [Discv4]
    to_service: mpsc::UnboundedSender<Discv4Command>,
    /// Receiver half of the commands channel for [Discv4]
    commands_rx: mpsc::UnboundedReceiver<Discv4Command>,
    /// All subscribers for table updates
    update_listeners: Vec<mpsc::Sender<DiscoveryUpdate>>,
    /// The interval when to trigger random lookups
    lookup_interval: Interval,
    /// Used to rotate targets to lookup
    lookup_rotator: LookupTargetRotator,
    /// Interval when to recheck active requests
    evict_expired_requests_interval: Interval,
    /// Interval when to resend pings.
    ping_interval: Interval,
    /// The interval at which to attempt resolving external IP again.
    resolve_external_ip_interval: Option<ResolveNatInterval>,
    /// How this services is configured
    config: Discv4Config,
    /// Buffered events populated during poll.
    queued_events: VecDeque<Discv4Event>,
    /// Keeps track of nodes from which we have received a `Pong` message.
    received_pongs: PongTable,
    /// Interval used to expire additionally tracked nodes
    expire_interval: Interval,
}

impl Discv4Service {
    /// Create a new instance for a bound [`UdpSocket`].
    pub(crate) fn new(
        socket: UdpSocket,
        local_address: SocketAddr,
        local_node_record: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
    ) -> Self {
        let socket = Arc::new(socket);
        let (ingress_tx, ingress_rx) = mpsc::channel(config.udp_ingress_message_buffer);
        let (egress_tx, egress_rx) = mpsc::channel(config.udp_egress_message_buffer);
        let mut tasks = JoinSet::<()>::new();

        let udp = Arc::clone(&socket);
        tasks.spawn(receive_loop(udp, ingress_tx, local_node_record.id));

        let udp = Arc::clone(&socket);
        tasks.spawn(send_loop(udp, egress_rx));

        let kbuckets = KBucketsTable::new(
            NodeKey::from(&local_node_record).into(),
            Duration::from_secs(60),
            MAX_NODES_PER_BUCKET,
            None,
            None,
        );

        let self_lookup_interval = tokio::time::interval(config.lookup_interval);

        // Wait `ping_interval` and then start pinging every `ping_interval` because we want to wait
        // for
        let ping_interval = tokio::time::interval_at(
            tokio::time::Instant::now() + config.ping_interval,
            config.ping_interval,
        );

        let evict_expired_requests_interval = tokio::time::interval_at(
            tokio::time::Instant::now() + config.request_timeout,
            config.request_timeout,
        );

        let lookup_rotator = if config.enable_dht_random_walk {
            LookupTargetRotator::default()
        } else {
            LookupTargetRotator::local_only()
        };

        // for EIP-868 construct an ENR
        let local_eip_868_enr = {
            let mut builder = EnrBuilder::new("v4");
            builder.ip(local_node_record.address);
            if local_node_record.address.is_ipv4() {
                builder.udp4(local_node_record.udp_port);
                builder.tcp4(local_node_record.tcp_port);
            } else {
                builder.udp6(local_node_record.udp_port);
                builder.tcp6(local_node_record.tcp_port);
            }

            for (key, val) in config.additional_eip868_rlp_pairs.iter() {
                builder.add_value_rlp(key, val.clone());
            }
            builder.build(&secret_key).expect("v4 is set")
        };

        let (to_service, commands_rx) = mpsc::unbounded_channel();

        let shared_node_record = Arc::new(Mutex::new(local_node_record));

        Discv4Service {
            local_address,
            local_eip_868_enr,
            local_node_record,
            shared_node_record,
            _socket: socket,
            kbuckets,
            secret_key,
            _tasks: tasks,
            ingress: ingress_rx,
            egress: egress_tx,
            queued_pings: Default::default(),
            pending_pings: Default::default(),
            pending_lookup: Default::default(),
            pending_find_nodes: Default::default(),
            pending_enr_requests: Default::default(),
            commands_rx,
            to_service,
            update_listeners: Vec::with_capacity(1),
            lookup_interval: self_lookup_interval,
            ping_interval,
            evict_expired_requests_interval,
            lookup_rotator,
            resolve_external_ip_interval: config.resolve_external_ip_interval(),
            config,
            queued_events: Default::default(),
            received_pongs: Default::default(),
            expire_interval: tokio::time::interval(EXPIRE_DURATION),
        }
    }

    /// Returns the frontend handle that can communicate with the service via commands.
    pub fn handle(&self) -> Discv4 {
        Discv4 {
            local_addr: self.local_address,
            to_service: self.to_service.clone(),
            node_record: self.shared_node_record.clone(),
        }
    }

    /// Returns the current enr sequence of the local record.
    fn enr_seq(&self) -> Option<u64> {
        (self.config.enable_eip868).then(|| self.local_eip_868_enr.seq())
    }

    /// Sets the [Interval] used for periodically looking up targets over the network
    pub fn set_lookup_interval(&mut self, duration: Duration) {
        self.lookup_interval = tokio::time::interval(duration);
    }

    /// Sets the given ip address as the node's external IP in the node record announced in
    /// discovery
    pub fn set_external_ip_addr(&mut self, external_ip: IpAddr) {
        if self.local_node_record.address != external_ip {
            debug!(target: "discv4", ?external_ip, "Updating external ip");
            self.local_node_record.address = external_ip;
            let _ = self.local_eip_868_enr.set_ip(external_ip, &self.secret_key);
            let mut lock = self.shared_node_record.lock();
            *lock = self.local_node_record;
            debug!(target: "discv4", enr=?self.local_eip_868_enr, "Updated local ENR");
        }
    }

    /// Returns the [PeerId] that identifies this node
    pub const fn local_peer_id(&self) -> &PeerId {
        &self.local_node_record.id
    }

    /// Returns the address of the UDP socket
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_address
    }

    /// Returns the ENR of this service.
    ///
    /// Note: this will include the external address if resolved.
    pub const fn local_enr(&self) -> NodeRecord {
        self.local_node_record
    }

    /// Returns mutable reference to ENR for testing.
    #[cfg(test)]
    pub fn local_enr_mut(&mut self) -> &mut NodeRecord {
        &mut self.local_node_record
    }

    /// Returns true if the given PeerId is currently in the bucket
    pub fn contains_node(&self, id: PeerId) -> bool {
        let key = kad_key(id);
        self.kbuckets.get_index(&key).is_some()
    }

    /// Bootstraps the local node to join the DHT.
    ///
    /// Bootstrapping is a multi-step operation that starts with a lookup of the local node's
    /// own ID in the DHT. This introduces the local node to the other nodes
    /// in the DHT and populates its routing table with the closest proven neighbours.
    ///
    /// This is similar to adding all bootnodes via [`Self::add_node`], but does not fire a
    /// [`DiscoveryUpdate::Added`] event for the given bootnodes. So boot nodes don't appear in the
    /// update stream, which is usually desirable, since bootnodes should not be connected to.
    ///
    /// If adding the configured bootnodes should result in a [`DiscoveryUpdate::Added`], see
    /// [`Self::add_all_nodes`].
    ///
    /// **Note:** This is a noop if there are no bootnodes.
    pub fn bootstrap(&mut self) {
        for record in self.config.bootstrap_nodes.clone() {
            debug!(target: "discv4", ?record, "pinging boot node");
            let key = kad_key(record.id);
            let entry = NodeEntry::new(record);

            // insert the boot node in the table
            match self.kbuckets.insert_or_update(
                &key,
                entry,
                NodeStatus {
                    state: ConnectionState::Disconnected,
                    direction: ConnectionDirection::Outgoing,
                },
            ) {
                InsertResult::Failed(_) => {}
                _ => {
                    self.try_ping(record, PingReason::InitialInsert);
                }
            }
        }
    }

    /// Spawns this services onto a new task
    ///
    /// Note: requires a running runtime
    pub fn spawn(mut self) -> JoinHandle<()> {
        tokio::task::spawn(async move {
            self.bootstrap();

            while let Some(event) = self.next().await {
                trace!(target: "discv4", ?event, "processed");
            }
            trace!(target: "discv4", "service terminated");
        })
    }

    /// Creates a new bounded channel for [`DiscoveryUpdate`]s.
    pub fn update_stream(&mut self) -> ReceiverStream<DiscoveryUpdate> {
        let (tx, rx) = mpsc::channel(512);
        self.update_listeners.push(tx);
        ReceiverStream::new(rx)
    }

    /// Looks up the local node in the DHT.
    pub fn lookup_self(&mut self) {
        self.lookup(self.local_node_record.id)
    }

    /// Looks up the given node in the DHT
    ///
    /// A FindNode packet requests information about nodes close to target. The target is a 64-byte
    /// secp256k1 public key. When FindNode is received, the recipient should reply with Neighbors
    /// packets containing the closest 16 nodes to target found in its local table.
    //
    // To guard against traffic amplification attacks, Neighbors replies should only be sent if the
    // sender of FindNode has been verified by the endpoint proof procedure.
    pub fn lookup(&mut self, target: PeerId) {
        self.lookup_with(target, None)
    }

    /// Starts the recursive lookup process for the given target, <https://github.com/ethereum/devp2p/blob/master/discv4.md#recursive-lookup>.
    ///
    /// At first the `ALPHA` (==3, defined concurrency factor) nodes that are closest to the target
    /// in the underlying DHT are selected to seed the lookup via `FindNode` requests. In the
    /// recursive step, the initiator resends FindNode to nodes it has learned about from previous
    /// queries.
    ///
    /// This takes an optional Sender through which all successfully discovered nodes are sent once
    /// the request has finished.
    fn lookup_with(&mut self, target: PeerId, tx: Option<NodeRecordSender>) {
        trace!(target: "discv4", ?target, "Starting lookup");
        let target_key = kad_key(target);

        // Start a lookup context with the 16 (MAX_NODES_PER_BUCKET) closest nodes
        let ctx = LookupContext::new(
            target_key.clone(),
            self.kbuckets
                .closest_values(&target_key)
                .filter(|node| {
                    node.value.has_endpoint_proof &&
                        !self.pending_find_nodes.contains_key(&node.key.preimage().0)
                })
                .take(MAX_NODES_PER_BUCKET)
                .map(|n| (target_key.distance(&n.key), n.value.record)),
            tx,
        );

        // From those 16, pick the 3 closest to start the concurrent lookup.
        let closest = ctx.closest(ALPHA);

        if closest.is_empty() && self.pending_find_nodes.is_empty() {
            // no closest nodes, and no lookup in progress: table is empty.
            // This could happen if all records were deleted from the table due to missed pongs
            // (e.g. connectivity problems over a long period of time, or issues during initial
            // bootstrapping) so we attempt to bootstrap again
            self.bootstrap();
            return
        }

        trace!(target: "discv4", ?target, num = closest.len(), "Start lookup closest nodes");

        for node in closest {
            self.find_node(&node, ctx.clone());
        }
    }

    /// Sends a new `FindNode` packet to the node with `target` as the lookup target.
    ///
    /// CAUTION: This expects there's a valid Endpoint proof to the given `node`.
    fn find_node(&mut self, node: &NodeRecord, ctx: LookupContext) {
        trace!(target: "discv4", ?node, lookup=?ctx.target(), "Sending FindNode");
        ctx.mark_queried(node.id);
        let id = ctx.target();
        let msg = Message::FindNode(FindNode { id, expire: self.find_node_expiration() });
        self.send_packet(msg, node.udp_addr());
        self.pending_find_nodes.insert(node.id, FindNodeRequest::new(ctx));
    }

    /// Notifies all listeners.
    ///
    /// Removes all listeners that are closed.
    fn notify(&mut self, update: DiscoveryUpdate) {
        self.update_listeners.retain_mut(|listener| match listener.try_send(update.clone()) {
            Ok(()) => true,
            Err(err) => match err {
                TrySendError::Full(_) => true,
                TrySendError::Closed(_) => false,
            },
        });
    }

    /// Adds the ip to the ban list indefinitely
    pub fn ban_ip(&mut self, ip: IpAddr) {
        self.config.ban_list.ban_ip(ip);
    }

    /// Adds the peer to the ban list indefinitely.
    pub fn ban_node(&mut self, node_id: PeerId) {
        self.remove_node(node_id);
        self.config.ban_list.ban_peer(node_id);
    }

    /// Adds the ip to the ban list until the given timestamp.
    pub fn ban_ip_until(&mut self, ip: IpAddr, until: Instant) {
        self.config.ban_list.ban_ip_until(ip, until);
    }

    /// Adds the peer to the ban list and bans it until the given timestamp
    pub fn ban_node_until(&mut self, node_id: PeerId, until: Instant) {
        self.remove_node(node_id);
        self.config.ban_list.ban_peer_until(node_id, until);
    }

    /// Removes a `node_id` from the routing table.
    ///
    /// This allows applications, for whatever reason, to remove nodes from the local routing
    /// table. Returns `true` if the node was in the table and `false` otherwise.
    pub fn remove_node(&mut self, node_id: PeerId) -> bool {
        let key = kad_key(node_id);
        let removed = self.kbuckets.remove(&key);
        if removed {
            debug!(target: "discv4", ?node_id, "removed node");
            self.notify(DiscoveryUpdate::Removed(node_id));
        }
        removed
    }

    /// Gets the number of entries that are considered connected.
    pub fn num_connected(&self) -> usize {
        self.kbuckets.buckets_iter().fold(0, |count, bucket| count + bucket.num_connected())
    }

    /// Check if the peer has a bond
    fn has_bond(&self, remote_id: PeerId, remote_ip: IpAddr) -> bool {
        if let Some(timestamp) = self.received_pongs.last_pong(remote_id, remote_ip) {
            if timestamp.elapsed() < self.config.bond_expiration {
                return true
            }
        }
        false
    }

    /// Update the entry on RE-ping
    ///
    /// On re-ping we check for a changed enr_seq if eip868 is enabled and when it changed we sent a
    /// followup request to retrieve the updated ENR
    fn update_on_reping(&mut self, record: NodeRecord, mut last_enr_seq: Option<u64>) {
        if record.id == self.local_node_record.id {
            return
        }

        // If EIP868 extension is disabled then we want to ignore this
        if !self.config.enable_eip868 {
            last_enr_seq = None;
        }

        let key = kad_key(record.id);
        let old_enr = match self.kbuckets.entry(&key) {
            kbucket::Entry::Present(mut entry, _) => {
                entry.value_mut().update_with_enr(last_enr_seq)
            }
            kbucket::Entry::Pending(mut entry, _) => entry.value().update_with_enr(last_enr_seq),
            _ => return,
        };

        // Check if ENR was updated
        match (last_enr_seq, old_enr) {
            (Some(new), Some(old)) => {
                if new > old {
                    self.send_enr_request(record);
                }
            }
            (Some(_), None) => {
                // got an ENR
                self.send_enr_request(record);
            }
            _ => {}
        };
    }

    /// Callback invoked when we receive a pong from the peer.
    fn update_on_pong(&mut self, record: NodeRecord, mut last_enr_seq: Option<u64>) {
        if record.id == *self.local_peer_id() {
            return
        }

        // If EIP868 extension is disabled then we want to ignore this
        if !self.config.enable_eip868 {
            last_enr_seq = None;
        }

        // if the peer included a enr seq in the pong then we can try to request the ENR of that
        // node
        let has_enr_seq = last_enr_seq.is_some();

        let key = kad_key(record.id);
        match self.kbuckets.entry(&key) {
            kbucket::Entry::Present(mut entry, old_status) => {
                // endpoint is now proven
                entry.value_mut().has_endpoint_proof = true;
                entry.value_mut().update_with_enr(last_enr_seq);

                if !old_status.is_connected() {
                    let _ = entry.update(ConnectionState::Connected, Some(old_status.direction));
                    debug!(target: "discv4", ?record, "added after successful endpoint proof");
                    self.notify(DiscoveryUpdate::Added(record));

                    if has_enr_seq {
                        // request the ENR of the node
                        self.send_enr_request(record);
                    }
                }
            }
            kbucket::Entry::Pending(mut entry, mut status) => {
                // endpoint is now proven
                entry.value().has_endpoint_proof = true;
                entry.value().update_with_enr(last_enr_seq);

                if !status.is_connected() {
                    status.state = ConnectionState::Connected;
                    let _ = entry.update(status);
                    debug!(target: "discv4", ?record, "added after successful endpoint proof");
                    self.notify(DiscoveryUpdate::Added(record));

                    if has_enr_seq {
                        // request the ENR of the node
                        self.send_enr_request(record);
                    }
                }
            }
            _ => {}
        };
    }

    /// Adds all nodes
    ///
    /// See [Self::add_node]
    pub fn add_all_nodes(&mut self, records: impl IntoIterator<Item = NodeRecord>) {
        for record in records.into_iter() {
            self.add_node(record);
        }
    }

    /// If the node's not in the table yet, this will add it to the table and start the endpoint
    /// proof by sending a ping to the node.
    ///
    /// Returns `true` if the record was added successfully, and `false` if the node is either
    /// already in the table or the record's bucket is full.
    pub fn add_node(&mut self, record: NodeRecord) -> bool {
        let key = kad_key(record.id);
        match self.kbuckets.entry(&key) {
            kbucket::Entry::Absent(entry) => {
                let node = NodeEntry::new(record);
                match entry.insert(
                    node,
                    NodeStatus {
                        direction: ConnectionDirection::Outgoing,
                        state: ConnectionState::Disconnected,
                    },
                ) {
                    BucketInsertResult::Inserted | BucketInsertResult::Pending { .. } => {
                        debug!(target: "discv4", ?record, "inserted new record");
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }

        // send the initial ping to the _new_ node
        self.try_ping(record, PingReason::InitialInsert);
        true
    }

    /// Encodes the packet, sends it and returns the hash.
    pub(crate) fn send_packet(&mut self, msg: Message, to: SocketAddr) -> B256 {
        let (payload, hash) = msg.encode(&self.secret_key);
        trace!(target: "discv4", r#type=?msg.msg_type(), ?to, ?hash, "sending packet");
        let _ = self.egress.try_send((payload, to)).map_err(|err| {
            debug!(
                target: "discv4",
                %err,
                "dropped outgoing packet",
            );
        });
        hash
    }

    /// Message handler for an incoming `Ping`
    fn on_ping(&mut self, ping: Ping, remote_addr: SocketAddr, remote_id: PeerId, hash: B256) {
        if self.is_expired(ping.expire) {
            // ping's expiration timestamp is in the past
            return
        }

        // create the record
        let record = NodeRecord {
            address: remote_addr.ip(),
            udp_port: remote_addr.port(),
            tcp_port: ping.from.tcp_port,
            id: remote_id,
        }
        .into_ipv4_mapped();

        let key = kad_key(record.id);

        // See also <https://github.com/ethereum/devp2p/blob/master/discv4.md#ping-packet-0x01>:
        // > If no communication with the sender of this ping has occurred within the last 12h, a
        // > ping should be sent in addition to pong in order to receive an endpoint proof.
        //
        // Note: we only mark if the node is absent because the `last 12h` condition is handled by
        // the ping interval
        let mut is_new_insert = false;
        let mut needs_bond = false;
        let mut is_proven = false;

        let old_enr = match self.kbuckets.entry(&key) {
            kbucket::Entry::Present(mut entry, _) => {
                is_proven = entry.value().has_endpoint_proof;
                entry.value_mut().update_with_enr(ping.enr_sq)
            }
            kbucket::Entry::Pending(mut entry, _) => {
                is_proven = entry.value().has_endpoint_proof;
                entry.value().update_with_enr(ping.enr_sq)
            }
            kbucket::Entry::Absent(entry) => {
                let mut node = NodeEntry::new(record);
                node.last_enr_seq = ping.enr_sq;

                match entry.insert(
                    node,
                    NodeStatus {
                        direction: ConnectionDirection::Incoming,
                        // mark as disconnected until endpoint proof established on pong
                        state: ConnectionState::Disconnected,
                    },
                ) {
                    BucketInsertResult::Inserted | BucketInsertResult::Pending { .. } => {
                        // mark as new insert if insert was successful
                        is_new_insert = true;
                    }
                    BucketInsertResult::Full => {
                        // we received a ping but the corresponding bucket for the peer is already
                        // full, we can't add any additional peers to that bucket, but we still want
                        // to emit an event that we discovered the node
                        trace!(target: "discv4", ?record, "discovered new record but bucket is full");
                        self.notify(DiscoveryUpdate::DiscoveredAtCapacity(record));
                        needs_bond = true;
                    }
                    BucketInsertResult::TooManyIncoming | BucketInsertResult::NodeExists => {
                        needs_bond = true;
                        // insert unsuccessful but we still want to send the pong
                    }
                    BucketInsertResult::FailedFilter => return,
                }

                None
            }
            kbucket::Entry::SelfEntry => return,
        };

        // send the pong first, but the PONG and optionally PING don't need to be send in a
        // particular order
        let pong = Message::Pong(Pong {
            // we use the actual address of the peer
            to: record.into(),
            echo: hash,
            expire: ping.expire,
            enr_sq: self.enr_seq(),
        });
        self.send_packet(pong, remote_addr);

        // if node was absent also send a ping to establish the endpoint proof from our end
        if is_new_insert {
            self.try_ping(record, PingReason::InitialInsert);
        } else if needs_bond {
            self.try_ping(record, PingReason::EstablishBond);
        } else if is_proven {
            // if node has been proven, this means we've received a pong and verified its endpoint
            // proof. We've also sent a pong above to verify our endpoint proof, so we can now
            // send our find_nodes request if PingReason::Lookup
            if let Some((_, ctx)) = self.pending_lookup.remove(&record.id) {
                if self.pending_find_nodes.contains_key(&record.id) {
                    // there's already another pending request, unmark it so the next round can
                    // try to send it
                    ctx.unmark_queried(record.id);
                } else {
                    self.find_node(&record, ctx);
                }
            }
        } else {
            // Request ENR if included in the ping
            match (ping.enr_sq, old_enr) {
                (Some(new), Some(old)) => {
                    if new > old {
                        self.send_enr_request(record);
                    }
                }
                (Some(_), None) => {
                    self.send_enr_request(record);
                }
                _ => {}
            };
        }
    }

    // Guarding function for [`Self::send_ping`] that applies pre-checks
    fn try_ping(&mut self, node: NodeRecord, reason: PingReason) {
        if node.id == *self.local_peer_id() {
            // don't ping ourselves
            return
        }

        if self.pending_pings.contains_key(&node.id) ||
            self.pending_find_nodes.contains_key(&node.id)
        {
            return
        }

        if self.queued_pings.iter().any(|(n, _)| n.id == node.id) {
            return
        }

        if self.pending_pings.len() < MAX_NODES_PING {
            self.send_ping(node, reason);
        } else {
            self.queued_pings.push_back((node, reason));
        }
    }

    /// Sends a ping message to the node's UDP address.
    ///
    /// Returns the echo hash of the ping message.
    pub(crate) fn send_ping(&mut self, node: NodeRecord, reason: PingReason) -> B256 {
        let remote_addr = node.udp_addr();
        let id = node.id;
        let ping = Ping {
            from: self.local_node_record.into(),
            to: node.into(),
            expire: self.ping_expiration(),
            enr_sq: self.enr_seq(),
        };
        trace!(target: "discv4", ?ping, "sending ping");
        let echo_hash = self.send_packet(Message::Ping(ping), remote_addr);

        self.pending_pings
            .insert(id, PingRequest { sent_at: Instant::now(), node, echo_hash, reason });
        echo_hash
    }

    /// Sends an enr request message to the node's UDP address.
    ///
    /// Returns the echo hash of the ping message.
    pub(crate) fn send_enr_request(&mut self, node: NodeRecord) {
        if !self.config.enable_eip868 {
            return
        }
        let remote_addr = node.udp_addr();
        let enr_request = EnrRequest { expire: self.enr_request_expiration() };

        trace!(target: "discv4", ?enr_request, "sending enr request");
        let echo_hash = self.send_packet(Message::EnrRequest(enr_request), remote_addr);

        self.pending_enr_requests
            .insert(node.id, EnrRequestState { sent_at: Instant::now(), echo_hash });
    }

    /// Message handler for an incoming `Pong`.
    fn on_pong(&mut self, pong: Pong, remote_addr: SocketAddr, remote_id: PeerId) {
        if self.is_expired(pong.expire) {
            return
        }

        let PingRequest { node, reason, .. } = match self.pending_pings.entry(remote_id) {
            Entry::Occupied(entry) => {
                {
                    let request = entry.get();
                    if request.echo_hash != pong.echo {
                        trace!(target: "discv4", from=?remote_addr, expected=?request.echo_hash, echo_hash=?pong.echo,"Got unexpected Pong");
                        return
                    }
                }
                entry.remove()
            }
            Entry::Vacant(_) => return,
        };

        // keep track of the pong
        self.received_pongs.on_pong(remote_id, remote_addr.ip());

        match reason {
            PingReason::InitialInsert => {
                self.update_on_pong(node, pong.enr_sq);
            }
            PingReason::EstablishBond => {
                // nothing to do here
            }
            PingReason::RePing => {
                self.update_on_reping(node, pong.enr_sq);
            }
            PingReason::Lookup(node, ctx) => {
                self.update_on_pong(node, pong.enr_sq);
                // insert node and assoc. lookup_context into the pending_lookup table to complete
                // our side of the endpoint proof verification.
                // Start the lookup timer here - and evict accordingly. Note that this is a separate
                // timer than the ping_request timer.
                self.pending_lookup.insert(node.id, (Instant::now(), ctx));
            }
        }
    }

    /// Handler for an incoming `FindNode` message
    fn on_find_node(&mut self, msg: FindNode, remote_addr: SocketAddr, node_id: PeerId) {
        if self.is_expired(msg.expire) {
            // expiration timestamp is in the past
            return
        }
        if node_id == *self.local_peer_id() {
            // ignore find node requests to ourselves
            return
        }

        if self.has_bond(node_id, remote_addr.ip()) {
            self.respond_closest(msg.id, remote_addr)
        }
    }

    /// Handler for incoming `EnrResponse` message
    fn on_enr_response(&mut self, msg: EnrResponse, remote_addr: SocketAddr, id: PeerId) {
        trace!(target: "discv4", ?remote_addr, ?msg, "received ENR response");
        if let Some(resp) = self.pending_enr_requests.remove(&id) {
            if resp.echo_hash == msg.request_hash {
                let key = kad_key(id);
                let fork_id = msg.eth_fork_id();
                let (record, old_fork_id) = match self.kbuckets.entry(&key) {
                    kbucket::Entry::Present(mut entry, _) => {
                        let id = entry.value_mut().update_with_fork_id(fork_id);
                        (entry.value().record, id)
                    }
                    kbucket::Entry::Pending(mut entry, _) => {
                        let id = entry.value().update_with_fork_id(fork_id);
                        (entry.value().record, id)
                    }
                    _ => return,
                };
                match (fork_id, old_fork_id) {
                    (Some(new), Some(old)) => {
                        if new != old {
                            self.notify(DiscoveryUpdate::EnrForkId(record, new))
                        }
                    }
                    (Some(new), None) => self.notify(DiscoveryUpdate::EnrForkId(record, new)),
                    _ => {}
                }
            }
        }
    }

    /// Handler for incoming `EnrRequest` message
    fn on_enr_request(
        &mut self,
        msg: EnrRequest,
        remote_addr: SocketAddr,
        id: PeerId,
        request_hash: B256,
    ) {
        if !self.config.enable_eip868 || self.is_expired(msg.expire) {
            return
        }

        if self.has_bond(id, remote_addr.ip()) {
            self.send_packet(
                Message::EnrResponse(EnrResponse {
                    request_hash,
                    enr: EnrWrapper::new(self.local_eip_868_enr.clone()),
                }),
                remote_addr,
            );
        }
    }

    /// Handler for incoming `Neighbours` messages that are handled if they're responses to
    /// `FindNode` requests.
    fn on_neighbours(&mut self, msg: Neighbours, remote_addr: SocketAddr, node_id: PeerId) {
        if self.is_expired(msg.expire) {
            // response is expired
            return
        }
        // check if this request was expected
        let ctx = match self.pending_find_nodes.entry(node_id) {
            Entry::Occupied(mut entry) => {
                {
                    let request = entry.get_mut();
                    // Mark the request as answered
                    request.answered = true;
                    let total = request.response_count + msg.nodes.len();

                    // Neighbours response is exactly 1 bucket (16 entries).
                    if total <= MAX_NODES_PER_BUCKET {
                        request.response_count = total;
                    } else {
                        trace!(target: "discv4", total, from=?remote_addr, "Received neighbors packet entries exceeds max nodes per bucket");
                        return
                    }
                };

                if entry.get().response_count == MAX_NODES_PER_BUCKET {
                    // node responding with a full bucket of records
                    let ctx = entry.remove().lookup_context;
                    ctx.mark_responded(node_id);
                    ctx
                } else {
                    entry.get().lookup_context.clone()
                }
            }
            Entry::Vacant(_) => {
                // received neighbours response without requesting it
                trace!(target: "discv4", from=?remote_addr, "Received unsolicited Neighbours");
                return
            }
        };

        // This is the recursive lookup step where we initiate new FindNode requests for new nodes
        // that were discovered.
        for node in msg.nodes.into_iter().map(NodeRecord::into_ipv4_mapped) {
            // prevent banned peers from being added to the context
            if self.config.ban_list.is_banned(&node.id, &node.address) {
                trace!(target: "discv4", peer_id=?node.id, ip=?node.address, "ignoring banned record");
                continue
            }

            ctx.add_node(node);
        }

        // get the next closest nodes, not yet queried nodes and start over.
        let closest =
            ctx.filter_closest(ALPHA, |node| !self.pending_find_nodes.contains_key(&node.id));

        for closest in closest {
            let key = kad_key(closest.id);
            match self.kbuckets.entry(&key) {
                BucketEntry::Absent(entry) => {
                    // the node's endpoint is not proven yet, so we need to ping it first, on
                    // success, we will add the node to the pending_lookup table, and wait to send
                    // back a Pong before initiating a FindNode request.
                    // In order to prevent that this node is selected again on subsequent responses,
                    // while the ping is still active, we always mark it as queried.
                    ctx.mark_queried(closest.id);
                    let node = NodeEntry::new(closest);
                    match entry.insert(
                        node,
                        NodeStatus {
                            direction: ConnectionDirection::Outgoing,
                            state: ConnectionState::Disconnected,
                        },
                    ) {
                        BucketInsertResult::Inserted | BucketInsertResult::Pending { .. } => {
                            // only ping if the node was added to the table
                            self.try_ping(closest, PingReason::Lookup(closest, ctx.clone()))
                        }
                        BucketInsertResult::Full => {
                            // new node but the node's bucket is already full
                            self.notify(DiscoveryUpdate::DiscoveredAtCapacity(closest))
                        }
                        _ => {}
                    }
                }
                BucketEntry::SelfEntry => {
                    // we received our own node entry
                }
                _ => self.find_node(&closest, ctx.clone()),
            }
        }
    }

    /// Sends a Neighbours packet for `target` to the given addr
    fn respond_closest(&mut self, target: PeerId, to: SocketAddr) {
        let key = kad_key(target);
        let expire = self.send_neighbours_expiration();
        let all_nodes = self.kbuckets.closest_values(&key).collect::<Vec<_>>();

        for nodes in all_nodes.chunks(SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS) {
            let nodes = nodes.iter().map(|node| node.value.record).collect::<Vec<NodeRecord>>();
            trace!(target: "discv4", len = nodes.len(), to=?to,"Sent neighbours packet");
            let msg = Message::Neighbours(Neighbours { nodes, expire });
            self.send_packet(msg, to);
        }
    }

    fn evict_expired_requests(&mut self, now: Instant) {
        self.pending_enr_requests.retain(|_node_id, enr_request| {
            now.duration_since(enr_request.sent_at) < self.config.ping_expiration
        });

        let mut failed_pings = Vec::new();
        self.pending_pings.retain(|node_id, ping_request| {
            if now.duration_since(ping_request.sent_at) > self.config.ping_expiration {
                failed_pings.push(*node_id);
                return false
            }
            true
        });

        debug!(target: "discv4", num=%failed_pings.len(), "evicting nodes due to failed pong");

        // remove nodes that failed to pong
        for node_id in failed_pings {
            self.remove_node(node_id);
        }

        let mut failed_lookups = Vec::new();
        self.pending_lookup.retain(|node_id, (lookup_sent_at, _)| {
            if now.duration_since(*lookup_sent_at) > self.config.ping_expiration {
                failed_lookups.push(*node_id);
                return false
            }
            true
        });
        debug!(target: "discv4", num=%failed_lookups.len(), "evicting nodes due to failed lookup");

        // remove nodes that failed the e2e lookup process, so we can restart it
        for node_id in failed_lookups {
            self.remove_node(node_id);
        }

        self.evict_failed_neighbours(now);
    }

    /// Handles failed responses to FindNode
    fn evict_failed_neighbours(&mut self, now: Instant) {
        let mut failed_neighbours = Vec::new();
        self.pending_find_nodes.retain(|node_id, find_node_request| {
            if now.duration_since(find_node_request.sent_at) > self.config.request_timeout {
                if !find_node_request.answered {
                    // node actually responded but with fewer entries than expected, but we don't
                    // treat this as an hard error since it responded.
                    failed_neighbours.push(*node_id);
                }
                return false
            }
            true
        });

        debug!(target: "discv4", num=%failed_neighbours.len(), "processing failed neighbours");

        for node_id in failed_neighbours {
            let key = kad_key(node_id);
            let failures = match self.kbuckets.entry(&key) {
                kbucket::Entry::Present(mut entry, _) => {
                    entry.value_mut().inc_failed_request();
                    entry.value().find_node_failures
                }
                kbucket::Entry::Pending(mut entry, _) => {
                    entry.value().inc_failed_request();
                    entry.value().find_node_failures
                }
                _ => continue,
            };

            // if the node failed to respond anything useful multiple times, remove the node from
            // the table, but only if there are enough other nodes in the bucket (bucket must be at
            // least half full)
            if failures > (self.config.max_find_node_failures as usize) {
                if let Some(bucket) = self.kbuckets.get_bucket(&key) {
                    if bucket.num_entries() < MAX_NODES_PER_BUCKET / 2 {
                        // skip half empty bucket
                        continue
                    }
                }
                self.remove_node(node_id);
            }
        }
    }

    /// Re-pings all nodes which endpoint proofs are considered expired: [``NodeEntry::is_expired]
    ///
    /// This will send a `Ping` to the nodes, if a node fails to respond with a `Pong` to renew the
    /// endpoint proof it will be removed from the table.
    fn re_ping_oldest(&mut self) {
        let mut nodes = self
            .kbuckets
            .iter_ref()
            .filter(|entry| entry.node.value.is_expired())
            .map(|n| n.node.value)
            .collect::<Vec<_>>();
        nodes.sort_by(|a, b| a.last_seen.cmp(&b.last_seen));
        let to_ping = nodes.into_iter().map(|n| n.record).take(MAX_NODES_PING).collect::<Vec<_>>();
        for node in to_ping {
            self.try_ping(node, PingReason::RePing)
        }
    }

    /// Returns true if the expiration timestamp is in the past.
    fn is_expired(&self, expiration: u64) -> bool {
        self.ensure_not_expired(expiration).is_err()
    }

    /// Validate that given timestamp is not expired.
    ///
    /// Note: this accepts the timestamp as u64 because this is used by the wire protocol, but the
    /// UNIX timestamp (number of non-leap seconds since January 1, 1970 0:00:00 UTC) is supposed to
    /// be an i64.
    ///
    /// Returns an error if:
    ///  - invalid UNIX timestamp (larger than i64::MAX)
    ///  - timestamp is expired (lower than current local UNIX timestamp)
    fn ensure_not_expired(&self, timestamp: u64) -> Result<(), ()> {
        // ensure the timestamp is a valid UNIX timestamp
        let _ = i64::try_from(timestamp).map_err(|_| ())?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if self.config.enforce_expiration_timestamps && timestamp < now {
            debug!(target: "discv4", "Expired packet");
            return Err(())
        }
        Ok(())
    }

    /// Pops buffered ping requests and sends them.
    fn ping_buffered(&mut self) {
        while self.pending_pings.len() < MAX_NODES_PING {
            match self.queued_pings.pop_front() {
                Some((next, reason)) => self.try_ping(next, reason),
                None => break,
            }
        }
    }

    fn ping_expiration(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + self.config.ping_expiration)
            .as_secs()
    }

    fn find_node_expiration(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + self.config.request_timeout)
            .as_secs()
    }

    fn enr_request_expiration(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + self.config.enr_expiration)
            .as_secs()
    }

    fn send_neighbours_expiration(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + self.config.neighbours_expiration)
            .as_secs()
    }

    /// Polls the socket and advances the state.
    ///
    /// To prevent traffic amplification attacks, implementations must verify that the sender of a
    /// query participates in the discovery protocol. The sender of a packet is considered verified
    /// if it has sent a valid Pong response with matching ping hash within the last 12 hours.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Discv4Event> {
        loop {
            // drain buffered events first
            if let Some(event) = self.queued_events.pop_front() {
                return Poll::Ready(event)
            }

            // trigger self lookup
            if self.config.enable_lookup {
                while self.lookup_interval.poll_tick(cx).is_ready() {
                    let target = self.lookup_rotator.next(&self.local_node_record.id);
                    self.lookup_with(target, None);
                }
            }

            // re-ping some peers
            if self.ping_interval.poll_tick(cx).is_ready() {
                let _ = self.ping_interval.poll_tick(cx);
                self.re_ping_oldest();
            }

            if let Some(Poll::Ready(Some(ip))) =
                self.resolve_external_ip_interval.as_mut().map(|r| r.poll_tick(cx))
            {
                self.set_external_ip_addr(ip);
            }

            // drain all incoming `Discv4` commands, this channel can never close
            while let Poll::Ready(Some(cmd)) = self.commands_rx.poll_recv(cx) {
                match cmd {
                    Discv4Command::Add(enr) => {
                        self.add_node(enr);
                    }
                    Discv4Command::Lookup { node_id, tx } => {
                        let node_id = node_id.unwrap_or(self.local_node_record.id);
                        self.lookup_with(node_id, tx);
                    }
                    Discv4Command::SetLookupInterval(duration) => {
                        self.set_lookup_interval(duration);
                    }
                    Discv4Command::Updates(tx) => {
                        let rx = self.update_stream();
                        let _ = tx.send(rx);
                    }
                    Discv4Command::BanPeer(node_id) => self.ban_node(node_id),
                    Discv4Command::Remove(node_id) => {
                        self.remove_node(node_id);
                    }
                    Discv4Command::Ban(node_id, ip) => {
                        self.ban_node(node_id);
                        self.ban_ip(ip);
                    }
                    Discv4Command::BanIp(ip) => {
                        self.ban_ip(ip);
                    }
                    Discv4Command::SetEIP868RLPPair { key, rlp } => {
                        debug!(target: "discv4", key=%String::from_utf8_lossy(&key), "Update EIP-868 extension pair");

                        let _ = self.local_eip_868_enr.insert_raw_rlp(key, rlp, &self.secret_key);
                    }
                    Discv4Command::SetTcpPort(port) => {
                        debug!(target: "discv4", %port, "Update tcp port");
                        self.local_node_record.tcp_port = port;
                        if self.local_node_record.address.is_ipv4() {
                            let _ = self.local_eip_868_enr.set_tcp4(port, &self.secret_key);
                        } else {
                            let _ = self.local_eip_868_enr.set_tcp6(port, &self.secret_key);
                        }
                    }

                    Discv4Command::Terminated => {
                        // terminate the service
                        self.queued_events.push_back(Discv4Event::Terminated);
                    }
                }
            }

            // restricts how many messages we process in a single poll before yielding back control
            let mut udp_message_budget = UDP_MESSAGE_POLL_LOOP_BUDGET;

            // process all incoming datagrams
            while let Poll::Ready(Some(event)) = self.ingress.poll_recv(cx) {
                match event {
                    IngressEvent::RecvError(err) => {
                        debug!(target: "discv4", %err, "failed to read datagram");
                    }
                    IngressEvent::BadPacket(from, err, data) => {
                        debug!(target: "discv4", ?from, %err, packet=?hex::encode(&data), "bad packet");
                    }
                    IngressEvent::Packet(remote_addr, Packet { msg, node_id, hash }) => {
                        trace!(target: "discv4", r#type=?msg.msg_type(), from=?remote_addr,"received packet");
                        let event = match msg {
                            Message::Ping(ping) => {
                                self.on_ping(ping, remote_addr, node_id, hash);
                                Discv4Event::Ping
                            }
                            Message::Pong(pong) => {
                                self.on_pong(pong, remote_addr, node_id);
                                Discv4Event::Pong
                            }
                            Message::FindNode(msg) => {
                                self.on_find_node(msg, remote_addr, node_id);
                                Discv4Event::FindNode
                            }
                            Message::Neighbours(msg) => {
                                self.on_neighbours(msg, remote_addr, node_id);
                                Discv4Event::Neighbours
                            }
                            Message::EnrRequest(msg) => {
                                self.on_enr_request(msg, remote_addr, node_id, hash);
                                Discv4Event::EnrRequest
                            }
                            Message::EnrResponse(msg) => {
                                self.on_enr_response(msg, remote_addr, node_id);
                                Discv4Event::EnrResponse
                            }
                        };

                        self.queued_events.push_back(event);
                    }
                }

                udp_message_budget -= 1;
                if udp_message_budget < 0 {
                    trace!(target: "discv4", budget=UDP_MESSAGE_POLL_LOOP_BUDGET, "exhausted message poll budget");
                    if self.queued_events.is_empty() {
                        // we've exceeded the message budget and have no events to process
                        // this will make sure we're woken up again
                        cx.waker().wake_by_ref();
                    }
                    break
                }
            }

            // try resending buffered pings
            self.ping_buffered();

            // evict expired nodes
            while self.evict_expired_requests_interval.poll_tick(cx).is_ready() {
                self.evict_expired_requests(Instant::now());
            }

            // evict expired nodes
            while self.expire_interval.poll_tick(cx).is_ready() {
                self.received_pongs.evict_expired(Instant::now(), EXPIRE_DURATION);
            }

            if self.queued_events.is_empty() {
                return Poll::Pending
            }
        }
    }
}

/// Endless future impl
impl Stream for Discv4Service {
    type Item = Discv4Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Poll the internal poll method
        match ready!(self.get_mut().poll(cx)) {
            // if the service is terminated, return None to terminate the stream
            Discv4Event::Terminated => Poll::Ready(None),
            // For any other event, return Poll::Ready(Some(event))
            ev => Poll::Ready(Some(ev)),
        }
    }
}

impl fmt::Debug for Discv4Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Discv4Service")
            .field("local_address", &self.local_address)
            .field("local_peer_id", &self.local_peer_id())
            .field("local_node_record", &self.local_node_record)
            .field("queued_pings", &self.queued_pings)
            .field("pending_lookup", &self.pending_lookup)
            .field("pending_find_nodes", &self.pending_find_nodes)
            .field("lookup_interval", &self.lookup_interval)
            .finish_non_exhaustive()
    }
}

/// The Event type the Service stream produces.
///
/// This is mainly used for testing purposes and represents messages the service processed
#[derive(Debug, Eq, PartialEq)]
pub enum Discv4Event {
    /// A `Ping` message was handled.
    Ping,
    /// A `Pong` message was handled.
    Pong,
    /// A `FindNode` message was handled.
    FindNode,
    /// A `Neighbours` message was handled.
    Neighbours,
    /// A `EnrRequest` message was handled.
    EnrRequest,
    /// A `EnrResponse` message was handled.
    EnrResponse,
    /// Service is being terminated
    Terminated,
}

/// Continuously reads new messages from the channel and writes them to the socket
pub(crate) async fn send_loop(udp: Arc<UdpSocket>, rx: EgressReceiver) {
    let mut stream = ReceiverStream::new(rx);
    while let Some((payload, to)) = stream.next().await {
        match udp.send_to(&payload, to).await {
            Ok(size) => {
                trace!(target: "discv4", ?to, ?size,"sent payload");
            }
            Err(err) => {
                debug!(target: "discv4", ?to, %err,"Failed to send datagram.");
            }
        }
    }
}

/// Continuously awaits new incoming messages and sends them back through the channel.
pub(crate) async fn receive_loop(udp: Arc<UdpSocket>, tx: IngressSender, local_id: PeerId) {
    let send = |event: IngressEvent| async {
        let _ = tx.send(event).await.map_err(|err| {
            debug!(
                target: "discv4",
                 %err,
                "failed send incoming packet",
            )
        });
    };

    let mut buf = [0; MAX_PACKET_SIZE];
    loop {
        let res = udp.recv_from(&mut buf).await;
        match res {
            Err(err) => {
                debug!(target: "discv4", %err, "Failed to read datagram.");
                send(IngressEvent::RecvError(err)).await;
            }
            Ok((read, remote_addr)) => {
                let packet = &buf[..read];
                match Message::decode(packet) {
                    Ok(packet) => {
                        if packet.node_id == local_id {
                            // received our own message
                            debug!(target: "discv4", ?remote_addr, "Received own packet.");
                            continue
                        }
                        send(IngressEvent::Packet(remote_addr, packet)).await;
                    }
                    Err(err) => {
                        debug!(target: "discv4", %err,"Failed to decode packet");
                        send(IngressEvent::BadPacket(remote_addr, err, packet.to_vec())).await
                    }
                }
            }
        }
    }
}

/// The commands sent from the frontend [Discv4] to the service [Discv4Service].
enum Discv4Command {
    Add(NodeRecord),
    SetTcpPort(u16),
    SetEIP868RLPPair { key: Vec<u8>, rlp: Bytes },
    Ban(PeerId, IpAddr),
    BanPeer(PeerId),
    BanIp(IpAddr),
    Remove(PeerId),
    Lookup { node_id: Option<PeerId>, tx: Option<NodeRecordSender> },
    SetLookupInterval(Duration),
    Updates(OneshotSender<ReceiverStream<DiscoveryUpdate>>),
    Terminated,
}

/// Event type receiver produces
#[derive(Debug)]
pub(crate) enum IngressEvent {
    /// Encountered an error when reading a datagram message.
    RecvError(io::Error),
    /// Received a bad message
    BadPacket(SocketAddr, DecodePacketError, Vec<u8>),
    /// Received a datagram from an address.
    Packet(SocketAddr, Packet),
}

/// Tracks a sent ping
#[derive(Debug)]
struct PingRequest {
    // Timestamp when the request was sent.
    sent_at: Instant,
    // Node to which the request was sent.
    node: NodeRecord,
    // Hash sent in the Ping request
    echo_hash: B256,
    /// Why this ping was sent.
    reason: PingReason,
}

/// Rotates the PeerId that is periodically looked up.
///
/// By selecting different targets, the lookups will be seeded with different ALPHA seed nodes.
#[derive(Debug)]
struct LookupTargetRotator {
    interval: usize,
    counter: usize,
}

// === impl LookupTargetRotator ===

impl LookupTargetRotator {
    /// Returns a rotator that always returns the local target.
    fn local_only() -> Self {
        Self { interval: 1, counter: 0 }
    }
}

impl Default for LookupTargetRotator {
    fn default() -> Self {
        Self {
            // every 4th lookup is our own node
            interval: 4,
            counter: 3,
        }
    }
}

impl LookupTargetRotator {
    /// this will return the next node id to lookup
    fn next(&mut self, local: &PeerId) -> PeerId {
        self.counter += 1;
        self.counter %= self.interval;
        if self.counter == 0 {
            return *local
        }
        PeerId::random()
    }
}

/// Tracks lookups across multiple `FindNode` requests.
///
/// If this type is dropped by all Clones, it will send all the discovered nodes to the listener, if
/// one is present.
#[derive(Clone, Debug)]
struct LookupContext {
    inner: Rc<LookupContextInner>,
}

impl LookupContext {
    /// Create new context for a recursive lookup
    fn new(
        target: discv5::Key<NodeKey>,
        nearest_nodes: impl IntoIterator<Item = (Distance, NodeRecord)>,
        listener: Option<NodeRecordSender>,
    ) -> Self {
        let closest_nodes = nearest_nodes
            .into_iter()
            .map(|(distance, record)| {
                (distance, QueryNode { record, queried: false, responded: false })
            })
            .collect();

        let inner = Rc::new(LookupContextInner {
            target,
            closest_nodes: RefCell::new(closest_nodes),
            listener,
        });
        Self { inner }
    }

    /// Returns the target of this lookup
    fn target(&self) -> PeerId {
        self.inner.target.preimage().0
    }

    fn closest(&self, num: usize) -> Vec<NodeRecord> {
        self.inner
            .closest_nodes
            .borrow()
            .iter()
            .filter(|(_, node)| !node.queried)
            .map(|(_, n)| n.record)
            .take(num)
            .collect()
    }

    /// Returns the closest nodes that have not been queried yet.
    fn filter_closest<P>(&self, num: usize, filter: P) -> Vec<NodeRecord>
    where
        P: FnMut(&NodeRecord) -> bool,
    {
        self.inner
            .closest_nodes
            .borrow()
            .iter()
            .filter(|(_, node)| !node.queried)
            .map(|(_, n)| n.record)
            .filter(filter)
            .take(num)
            .collect()
    }

    /// Inserts the node if it's missing
    fn add_node(&self, record: NodeRecord) {
        let distance = self.inner.target.distance(&kad_key(record.id));
        let mut closest = self.inner.closest_nodes.borrow_mut();
        if let btree_map::Entry::Vacant(entry) = closest.entry(distance) {
            entry.insert(QueryNode { record, queried: false, responded: false });
        }
    }

    fn set_queried(&self, id: PeerId, val: bool) {
        if let Some((_, node)) =
            self.inner.closest_nodes.borrow_mut().iter_mut().find(|(_, node)| node.record.id == id)
        {
            node.queried = val;
        }
    }

    /// Marks the node as queried
    fn mark_queried(&self, id: PeerId) {
        self.set_queried(id, true)
    }

    /// Marks the node as not queried
    fn unmark_queried(&self, id: PeerId) {
        self.set_queried(id, false)
    }

    /// Marks the node as responded
    fn mark_responded(&self, id: PeerId) {
        if let Some((_, node)) =
            self.inner.closest_nodes.borrow_mut().iter_mut().find(|(_, node)| node.record.id == id)
        {
            node.responded = true;
        }
    }
}

// SAFETY: The [`Discv4Service`] is intended to be spawned as task which requires `Send`.
// The `LookupContext` is shared by all active `FindNode` requests that are part of the lookup step.
// Which can modify the context. The shared context is only ever accessed mutably when a `Neighbour`
// response is processed and all Clones are stored inside [`Discv4Service`], in other words it is
// guaranteed that there's only 1 owner ([`Discv4Service`]) of all possible [`Rc`] clones of
// [`LookupContext`].
unsafe impl Send for LookupContext {}
#[derive(Debug)]
struct LookupContextInner {
    /// The target to lookup.
    target: discv5::Key<NodeKey>,
    /// The closest nodes
    closest_nodes: RefCell<BTreeMap<Distance, QueryNode>>,
    /// A listener for all the nodes retrieved in this lookup
    ///
    /// This is present if the lookup was triggered manually via [Discv4] and we want to return all
    /// the nodes once the lookup finishes.
    listener: Option<NodeRecordSender>,
}

impl Drop for LookupContextInner {
    fn drop(&mut self) {
        if let Some(tx) = self.listener.take() {
            // there's only 1 instance shared across `FindNode` requests, if this is dropped then
            // all requests finished, and we can send all results back
            let nodes = self
                .closest_nodes
                .take()
                .into_values()
                .filter(|node| node.responded)
                .map(|node| node.record)
                .collect();
            let _ = tx.send(nodes);
        }
    }
}

/// Tracks the state of a recursive lookup step
#[derive(Debug, Clone, Copy)]
struct QueryNode {
    record: NodeRecord,
    queried: bool,
    responded: bool,
}

#[derive(Debug)]
struct FindNodeRequest {
    // Timestamp when the request was sent.
    sent_at: Instant,
    // Number of items sent by the node
    response_count: usize,
    // Whether the request has been answered yet.
    answered: bool,
    /// Response buffer
    lookup_context: LookupContext,
}

// === impl FindNodeRequest ===

impl FindNodeRequest {
    fn new(resp: LookupContext) -> Self {
        Self { sent_at: Instant::now(), response_count: 0, answered: false, lookup_context: resp }
    }
}

#[derive(Debug)]
struct EnrRequestState {
    // Timestamp when the request was sent.
    sent_at: Instant,
    // Hash sent in the Ping request
    echo_hash: B256,
}

/// Stored node info.
#[derive(Debug, Clone, Eq, PartialEq)]
struct NodeEntry {
    /// Node record info.
    record: NodeRecord,
    /// Timestamp of last pong.
    last_seen: Instant,
    /// Last enr seq we retrieved via a ENR request.
    last_enr_seq: Option<u64>,
    /// ForkId if retrieved via ENR requests.
    fork_id: Option<ForkId>,
    /// Counter for failed findNode requests.
    find_node_failures: usize,
    /// Whether the endpoint of the peer is proven.
    has_endpoint_proof: bool,
}

// === impl NodeEntry ===

impl NodeEntry {
    /// Creates a new, unpopulated entry
    fn new(record: NodeRecord) -> Self {
        Self {
            record,
            last_seen: Instant::now(),
            last_enr_seq: None,
            fork_id: None,
            find_node_failures: 0,
            has_endpoint_proof: false,
        }
    }

    #[cfg(test)]
    fn new_proven(record: NodeRecord) -> Self {
        let mut node = Self::new(record);
        node.has_endpoint_proof = true;
        node
    }

    /// Updates the last timestamp and sets the enr seq
    fn update_with_enr(&mut self, last_enr_seq: Option<u64>) -> Option<u64> {
        self.update_now(|s| std::mem::replace(&mut s.last_enr_seq, last_enr_seq))
    }

    /// Increases the failed request counter
    fn inc_failed_request(&mut self) {
        self.find_node_failures += 1;
    }

    /// Updates the last timestamp and sets the enr seq
    fn update_with_fork_id(&mut self, fork_id: Option<ForkId>) -> Option<ForkId> {
        self.update_now(|s| std::mem::replace(&mut s.fork_id, fork_id))
    }

    /// Updates the last_seen timestamp and calls the closure
    fn update_now<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.last_seen = Instant::now();
        f(self)
    }
}

// === impl NodeEntry ===

impl NodeEntry {
    /// Returns true if the node should be re-pinged.
    fn is_expired(&self) -> bool {
        self.last_seen.elapsed() > ENDPOINT_PROOF_EXPIRATION
    }
}

/// Represents why a ping is issued
#[derive(Debug)]
enum PingReason {
    /// Initial ping to a previously unknown peer that was inserted into the table.
    InitialInsert,
    /// Initial ping to a previously unknown peer that didn't fit into the table. But we still want
    /// to establish a bond.
    EstablishBond,
    /// Re-ping a peer.
    RePing,
    /// Part of a lookup to ensure endpoint is proven before we can send a FindNode request.
    Lookup(NodeRecord, LookupContext),
}

/// Represents node related updates state changes in the underlying node table
#[derive(Debug, Clone)]
pub enum DiscoveryUpdate {
    /// A new node was discovered _and_ added to the table.
    Added(NodeRecord),
    /// A new node was discovered but _not_ added to the table because it is currently full.
    DiscoveredAtCapacity(NodeRecord),
    /// Received a [`ForkId`] via EIP-868 for the given [`NodeRecord`].
    EnrForkId(NodeRecord, ForkId),
    /// Node that was removed from the table
    Removed(PeerId),
    /// A series of updates
    Batch(Vec<DiscoveryUpdate>),
}

/// Represents a forward-compatible ENR entry for including the forkid in a node record via
/// EIP-868. Forward compatibility is achieved by allowing trailing fields.
///
/// See:
/// <https://github.com/ethereum/go-ethereum/blob/9244d5cd61f3ea5a7645fdf2a1a96d53421e412f/eth/protocols/eth/discovery.go#L27-L38>
///
/// for how geth implements ForkId values and forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[rlp(trailing)]
pub struct EnrForkIdEntry {
    /// The inner forkid
    pub fork_id: ForkId,
}

impl From<ForkId> for EnrForkIdEntry {
    fn from(fork_id: ForkId) -> Self {
        Self { fork_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{create_discv4, create_discv4_with_config, rng_endpoint, rng_record};
    use alloy_rlp::{Decodable, Encodable};
    use rand::{thread_rng, Rng};
    use reth_primitives::{hex, mainnet_nodes, ForkHash};
    use std::future::poll_fn;

    #[tokio::test]
    async fn test_configured_enr_forkid_entry() {
        let fork: ForkId = ForkId { hash: ForkHash([220, 233, 108, 45]), next: 0u64 };
        let mut disc_conf = Discv4Config::default();
        disc_conf.add_eip868_pair("eth", EnrForkIdEntry::from(fork));
        let (_discv4, service) = create_discv4_with_config(disc_conf).await;
        let eth = service.local_eip_868_enr.get_raw_rlp(b"eth").unwrap();
        let fork_entry_id = EnrForkIdEntry::decode(&mut &eth[..]).unwrap();

        let raw: [u8; 8] = [0xc7, 0xc6, 0x84, 0xdc, 0xe9, 0x6c, 0x2d, 0x80];
        let decoded = EnrForkIdEntry::decode(&mut &raw[..]).unwrap();
        let expected = EnrForkIdEntry {
            fork_id: ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 },
        };
        assert_eq!(expected, fork_entry_id);
        assert_eq!(expected, decoded);
    }

    #[test]
    fn test_enr_forkid_entry_decode() {
        let raw: [u8; 8] = [0xc7, 0xc6, 0x84, 0xdc, 0xe9, 0x6c, 0x2d, 0x80];
        let decoded = EnrForkIdEntry::decode(&mut &raw[..]).unwrap();
        let expected = EnrForkIdEntry {
            fork_id: ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 },
        };
        assert_eq!(expected, decoded);
    }

    #[test]
    fn test_enr_forkid_entry_encode() {
        let original = EnrForkIdEntry {
            fork_id: ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 },
        };
        let mut encoded = Vec::new();
        original.encode(&mut encoded);
        let expected: [u8; 8] = [0xc7, 0xc6, 0x84, 0xdc, 0xe9, 0x6c, 0x2d, 0x80];
        assert_eq!(&expected[..], encoded.as_slice());
    }

    #[test]
    fn test_local_rotator() {
        let id = PeerId::random();
        let mut rotator = LookupTargetRotator::local_only();
        assert_eq!(rotator.next(&id), id);
        assert_eq!(rotator.next(&id), id);
    }

    #[test]
    fn test_rotator() {
        let id = PeerId::random();
        let mut rotator = LookupTargetRotator::default();
        assert_eq!(rotator.next(&id), id);
        assert_ne!(rotator.next(&id), id);
        assert_ne!(rotator.next(&id), id);
        assert_ne!(rotator.next(&id), id);
        assert_eq!(rotator.next(&id), id);
    }

    #[tokio::test]
    async fn test_pending_ping() {
        let (_, mut service) = create_discv4().await;

        let local_addr = service.local_addr();

        let mut num_inserted = 0;
        for _ in 0..MAX_NODES_PING {
            let node = NodeRecord::new(local_addr, PeerId::random());
            if service.add_node(node) {
                num_inserted += 1;
                assert!(service.pending_pings.contains_key(&node.id));
                assert_eq!(service.pending_pings.len(), num_inserted);
            }
        }
    }

    // Bootstraps with mainnet boot nodes
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_mainnet_lookup() {
        reth_tracing::init_test_tracing();
        let fork_id = ForkId { hash: ForkHash(hex!("743f3d89")), next: 16191202 };

        let all_nodes = mainnet_nodes();
        let config = Discv4Config::builder()
            .add_boot_nodes(all_nodes)
            .lookup_interval(Duration::from_secs(1))
            .add_eip868_pair("eth", fork_id)
            .build();
        let (_discv4, mut service) = create_discv4_with_config(config).await;

        let mut updates = service.update_stream();

        let _handle = service.spawn();

        let mut table = HashMap::new();
        while let Some(update) = updates.next().await {
            match update {
                DiscoveryUpdate::EnrForkId(record, fork_id) => {
                    println!("{record:?}, {fork_id:?}");
                }
                DiscoveryUpdate::Added(record) => {
                    table.insert(record.id, record);
                }
                DiscoveryUpdate::Removed(id) => {
                    table.remove(&id);
                }
                _ => {}
            }
            println!("total peers {}", table.len());
        }
    }

    #[tokio::test]
    async fn test_mapped_ipv4() {
        reth_tracing::init_test_tracing();
        let mut rng = thread_rng();
        let config = Discv4Config::builder().build();
        let (_discv4, mut service) = create_discv4_with_config(config).await;

        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();
        let v6 = v4.to_ipv6_mapped();
        let addr: SocketAddr = (v6, DEFAULT_DISCOVERY_PORT).into();

        let ping = Ping {
            from: rng_endpoint(&mut rng),
            to: rng_endpoint(&mut rng),
            expire: service.ping_expiration(),
            enr_sq: Some(rng.gen()),
        };

        let id = PeerId::random_with(&mut rng);
        service.on_ping(ping, addr, id, rng.gen());

        let key = kad_key(id);
        match service.kbuckets.entry(&key) {
            kbucket::Entry::Present(entry, _) => {
                let node_addr = entry.value().record.address;
                assert!(node_addr.is_ipv4());
                assert_eq!(node_addr, IpAddr::from(v4));
            }
            _ => unreachable!(),
        };
    }

    #[tokio::test]
    async fn test_respect_ping_expiration() {
        reth_tracing::init_test_tracing();
        let mut rng = thread_rng();
        let config = Discv4Config::builder().build();
        let (_discv4, mut service) = create_discv4_with_config(config).await;

        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();
        let v6 = v4.to_ipv6_mapped();
        let addr: SocketAddr = (v6, DEFAULT_DISCOVERY_PORT).into();

        let ping = Ping {
            from: rng_endpoint(&mut rng),
            to: rng_endpoint(&mut rng),
            expire: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() - 1,
            enr_sq: Some(rng.gen()),
        };

        let id = PeerId::random_with(&mut rng);
        service.on_ping(ping, addr, id, rng.gen());

        let key = kad_key(id);
        match service.kbuckets.entry(&key) {
            kbucket::Entry::Absent(_) => {}
            _ => unreachable!(),
        };
    }

    #[tokio::test]
    async fn test_single_lookups() {
        reth_tracing::init_test_tracing();

        let config = Discv4Config::builder().build();
        let (_discv4, mut service) = create_discv4_with_config(config.clone()).await;

        let id = PeerId::random();
        let key = kad_key(id);
        let record = NodeRecord::new("0.0.0.0:0".parse().unwrap(), id);

        let _ = service.kbuckets.insert_or_update(
            &key,
            NodeEntry::new_proven(record),
            NodeStatus {
                direction: ConnectionDirection::Incoming,
                state: ConnectionState::Connected,
            },
        );

        service.lookup_self();
        assert_eq!(service.pending_find_nodes.len(), 1);

        poll_fn(|cx| {
            let _ = service.poll(cx);
            assert_eq!(service.pending_find_nodes.len(), 1);

            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn test_on_neighbours_recursive_lookup() {
        reth_tracing::init_test_tracing();

        let config = Discv4Config::builder().build();
        let (_discv4, mut service) = create_discv4_with_config(config.clone()).await;
        let (_discv4, mut service2) = create_discv4_with_config(config).await;

        let id = PeerId::random();
        let key = kad_key(id);
        let record = NodeRecord::new("0.0.0.0:0".parse().unwrap(), id);

        let _ = service.kbuckets.insert_or_update(
            &key,
            NodeEntry::new_proven(record),
            NodeStatus {
                direction: ConnectionDirection::Incoming,
                state: ConnectionState::Connected,
            },
        );
        // Needed in this test to populate self.pending_find_nodes for as a prereq to a valid
        // on_neighbours request
        service.lookup_self();
        assert_eq!(service.pending_find_nodes.len(), 1);

        poll_fn(|cx| {
            let _ = service.poll(cx);
            assert_eq!(service.pending_find_nodes.len(), 1);

            Poll::Ready(())
        })
        .await;

        let expiry = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() +
            10000000000000;
        let msg = Neighbours { nodes: vec![service2.local_node_record], expire: expiry };
        service.on_neighbours(msg, record.tcp_addr(), id);
        // wait for the processed ping
        let event = poll_fn(|cx| service2.poll(cx)).await;
        assert_eq!(event, Discv4Event::Ping);
        // assert that no find_node req has been added here on top of the initial one, since both
        // sides of the endpoint proof is not completed here
        assert_eq!(service.pending_find_nodes.len(), 1);
        // we now wait for PONG
        let event = poll_fn(|cx| service.poll(cx)).await;
        assert_eq!(event, Discv4Event::Pong);
        // Ideally we want to assert against service.pending_lookup.len() here - but because the
        // service2 sends Pong and Ping consecutivley on_ping(), the pending_lookup table gets
        // drained almost immediately - and no way to grab the handle to its intermediary state here
        // :(
        let event = poll_fn(|cx| service.poll(cx)).await;
        assert_eq!(event, Discv4Event::Ping);
        // assert that we've added the find_node req here after both sides of the endpoint proof is
        // done
        assert_eq!(service.pending_find_nodes.len(), 2);
    }

    #[tokio::test]
    async fn test_no_local_in_closest() {
        reth_tracing::init_test_tracing();

        let config = Discv4Config::builder().build();
        let (_discv4, mut service) = create_discv4_with_config(config).await;

        let target_key = kad_key(PeerId::random());

        let id = PeerId::random();
        let key = kad_key(id);
        let record = NodeRecord::new("0.0.0.0:0".parse().unwrap(), id);

        let _ = service.kbuckets.insert_or_update(
            &key,
            NodeEntry::new(record),
            NodeStatus {
                direction: ConnectionDirection::Incoming,
                state: ConnectionState::Connected,
            },
        );

        let closest = service
            .kbuckets
            .closest_values(&target_key)
            .map(|n| n.value.record)
            .take(MAX_NODES_PER_BUCKET)
            .collect::<Vec<_>>();

        assert_eq!(closest.len(), 1);
        assert!(!closest.iter().any(|r| r.id == *service.local_peer_id()));
    }

    #[tokio::test]
    async fn test_random_lookup() {
        reth_tracing::init_test_tracing();

        let config = Discv4Config::builder().build();
        let (_discv4, mut service) = create_discv4_with_config(config).await;

        let target = PeerId::random();

        let id = PeerId::random();
        let key = kad_key(id);
        let record = NodeRecord::new("0.0.0.0:0".parse().unwrap(), id);

        let _ = service.kbuckets.insert_or_update(
            &key,
            NodeEntry::new_proven(record),
            NodeStatus {
                direction: ConnectionDirection::Incoming,
                state: ConnectionState::Connected,
            },
        );

        service.lookup(target);
        assert_eq!(service.pending_find_nodes.len(), 1);

        let ctx = service.pending_find_nodes.values().next().unwrap().lookup_context.clone();

        assert_eq!(ctx.target(), target);
        assert_eq!(ctx.inner.closest_nodes.borrow().len(), 1);

        ctx.add_node(record);
        assert_eq!(ctx.inner.closest_nodes.borrow().len(), 1);
    }

    #[tokio::test]
    async fn test_service_commands() {
        reth_tracing::init_test_tracing();

        let config = Discv4Config::builder().build();
        let (discv4, mut service) = create_discv4_with_config(config).await;

        service.lookup_self();

        let _handle = service.spawn();
        discv4.send_lookup_self();
        let _ = discv4.lookup_self().await;
    }

    // sends a PING packet with wrong 'to' field and expects a PONG response.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_check_wrong_to() {
        reth_tracing::init_test_tracing();

        let config = Discv4Config::builder().external_ip_resolver(None).build();
        let (_discv4, mut service_1) = create_discv4_with_config(config.clone()).await;
        let (_discv4, mut service_2) = create_discv4_with_config(config).await;

        // ping node 2 with wrong to field
        let mut ping = Ping {
            from: service_1.local_node_record.into(),
            to: service_2.local_node_record.into(),
            expire: service_1.ping_expiration(),
            enr_sq: service_1.enr_seq(),
        };
        ping.to.address = "192.0.2.0".parse().unwrap();

        let echo_hash = service_1.send_packet(Message::Ping(ping), service_2.local_addr());
        let ping_request = PingRequest {
            sent_at: Instant::now(),
            node: service_2.local_node_record,
            echo_hash,
            reason: PingReason::InitialInsert,
        };
        service_1.pending_pings.insert(*service_2.local_peer_id(), ping_request);

        // wait for the processed ping
        let event = poll_fn(|cx| service_2.poll(cx)).await;
        assert_eq!(event, Discv4Event::Ping);

        // we now wait for PONG
        let event = poll_fn(|cx| service_1.poll(cx)).await;
        assert_eq!(event, Discv4Event::Pong);
        // followed by a ping
        let event = poll_fn(|cx| service_1.poll(cx)).await;
        assert_eq!(event, Discv4Event::Ping);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_check_ping_pong() {
        reth_tracing::init_test_tracing();

        let config = Discv4Config::builder().external_ip_resolver(None).build();
        let (_discv4, mut service_1) = create_discv4_with_config(config.clone()).await;
        let (_discv4, mut service_2) = create_discv4_with_config(config).await;

        // send ping from 1 -> 2
        service_1.add_node(service_2.local_node_record);

        // wait for the processed ping
        let event = poll_fn(|cx| service_2.poll(cx)).await;
        assert_eq!(event, Discv4Event::Ping);

        // node is now in the table but not connected yet
        let key1 = kad_key(*service_1.local_peer_id());
        match service_2.kbuckets.entry(&key1) {
            kbucket::Entry::Present(_entry, status) => {
                assert!(!status.is_connected());
            }
            _ => unreachable!(),
        }

        // we now wait for PONG
        let event = poll_fn(|cx| service_1.poll(cx)).await;
        assert_eq!(event, Discv4Event::Pong);

        // endpoint is proven
        let key2 = kad_key(*service_2.local_peer_id());
        match service_1.kbuckets.entry(&key2) {
            kbucket::Entry::Present(_entry, status) => {
                assert!(status.is_connected());
            }
            _ => unreachable!(),
        }

        // we now wait for the PING initiated by 2
        let event = poll_fn(|cx| service_1.poll(cx)).await;
        assert_eq!(event, Discv4Event::Ping);

        // we now wait for PONG
        let event = poll_fn(|cx| service_2.poll(cx)).await;

        match event {
            Discv4Event::EnrRequest => {
                // since we support enr in the ping it may also request the enr
                let event = poll_fn(|cx| service_2.poll(cx)).await;
                match event {
                    Discv4Event::EnrRequest => {
                        let event = poll_fn(|cx| service_2.poll(cx)).await;
                        assert_eq!(event, Discv4Event::Pong);
                    }
                    Discv4Event::Pong => {}
                    _ => {
                        unreachable!()
                    }
                }
            }
            Discv4Event::Pong => {}
            ev => unreachable!("{ev:?}"),
        }

        // endpoint is proven
        match service_2.kbuckets.entry(&key1) {
            kbucket::Entry::Present(_entry, status) => {
                assert!(status.is_connected());
            }
            ev => unreachable!("{ev:?}"),
        }
    }

    #[test]
    fn test_insert() {
        let local_node_record = rng_record(&mut rand::thread_rng());
        let mut kbuckets: KBucketsTable<NodeKey, NodeEntry> = KBucketsTable::new(
            NodeKey::from(&local_node_record).into(),
            Duration::from_secs(60),
            MAX_NODES_PER_BUCKET,
            None,
            None,
        );

        let new_record = rng_record(&mut rand::thread_rng());
        let key = kad_key(new_record.id);
        match kbuckets.entry(&key) {
            kbucket::Entry::Absent(entry) => {
                let node = NodeEntry::new(new_record);
                let _ = entry.insert(
                    node,
                    NodeStatus {
                        direction: ConnectionDirection::Outgoing,
                        state: ConnectionState::Disconnected,
                    },
                );
            }
            _ => {
                unreachable!()
            }
        };
        match kbuckets.entry(&key) {
            kbucket::Entry::Present(_, _) => {}
            _ => {
                unreachable!()
            }
        }
    }
}
