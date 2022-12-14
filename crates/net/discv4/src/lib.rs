#![warn(missing_docs, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

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
use crate::{
    error::{DecodePacketError, Discv4Error},
    node::{kad_key, NodeKey},
    proto::{FindNode, Message, Neighbours, Packet, Ping, Pong},
};
use bytes::Bytes;
use discv5::{
    kbucket::{
        Distance, Entry as BucketEntry, FailureReason, InsertResult, KBucketsTable, NodeStatus,
        MAX_NODES_PER_BUCKET,
    },
    ConnectionDirection, ConnectionState,
};
use reth_primitives::{PeerId, H256};
use secp256k1::SecretKey;
use std::{
    cell::RefCell,
    collections::{btree_map, hash_map::Entry, BTreeMap, HashMap, VecDeque},
    io,
    net::{IpAddr, SocketAddr},
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
use tracing::{debug, trace, warn};

pub mod bootnodes;
pub mod error;
mod proto;

mod config;
pub use config::{Discv4Config, Discv4ConfigBuilder};
mod node;
pub use node::NodeRecord;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

/// reexport to get public ip.
pub use public_ip;

/// The default port for discv4 via UDP
///
/// Note: the default TCP port is the same.
pub const DEFAULT_DISCOVERY_PORT: u16 = 30303;

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
const SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS: usize = (MAX_PACKET_SIZE - 109) / 91;

/// The timeout used to identify expired nodes
const NODE_LAST_SEEN_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

type EgressSender = mpsc::Sender<(Bytes, SocketAddr)>;
type EgressReceiver = mpsc::Receiver<(Bytes, SocketAddr)>;

pub(crate) type IngressSender = mpsc::Sender<IngressEvent>;
pub(crate) type IngressReceiver = mpsc::Receiver<IngressEvent>;

type NodeRecordSender = OneshotSender<Vec<NodeRecord>>;

/// The Discv4 frontend
#[derive(Debug, Clone)]
pub struct Discv4 {
    /// The address of the udp socket
    local_addr: SocketAddr,
    /// channel to send commands over to the service
    to_service: mpsc::Sender<Discv4Command>,
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

    /// Binds a new UdpSocket and creates the service
    ///
    /// ```
    /// # use std::io;
    /// use std::net::SocketAddr;
    /// use std::str::FromStr;
    /// use rand::thread_rng;
    /// use secp256k1::SECP256K1;
    /// use reth_primitives::PeerId;
    /// use reth_discv4::{Discv4, Discv4Config, NodeRecord};
    /// # async fn t() -> io::Result<()> {
    /// // generate a (random) keypair
    ///  let mut rng = thread_rng();
    ///  let (secret_key, pk) = SECP256K1.generate_keypair(&mut rng);
    ///  let id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
    ///
    ///  let socket = SocketAddr::from_str("0.0.0.0:0").unwrap();
    ///  let local_enr = NodeRecord {
    ///      address: socket.ip(),
    ///      tcp_port: socket.port(),
    ///      udp_port: socket.port(),
    ///      id,
    ///  };
    ///  let config = Discv4Config::default();
    ///
    ///  let(discv4, mut service) = Discv4::bind(socket, local_enr, secret_key, config).await.unwrap();
    ///
    ///   // get an update strea
    ///   let updates = service.update_stream();
    ///
    ///   let _handle = service.spawn();
    ///
    ///   // lookup the local node in the DHT
    ///   let _discovered = discv4.lookup_self().await.unwrap();
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bind(
        local_address: SocketAddr,
        mut local_enr: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
    ) -> io::Result<(Self, Discv4Service)> {
        let socket = UdpSocket::bind(local_address).await?;
        let local_addr = socket.local_addr()?;
        local_enr.udp_port = local_addr.port();
        trace!( target : "discv4",  ?local_addr,"opened UDP socket");

        // We don't expect many commands, so the buffer can be quite small here.
        let (to_service, rx) = mpsc::channel(5);
        let service =
            Discv4Service::new(socket, local_addr, local_enr, secret_key, config, Some(rx));
        let discv4 = Discv4 { local_addr, to_service };
        Ok((discv4, service))
    }

    /// Returns the address of the UDP socket.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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

    /// Looks up the given node id
    pub async fn lookup(&self, node_id: PeerId) -> Result<Vec<NodeRecord>, Discv4Error> {
        self.lookup_node(Some(node_id)).await
    }

    async fn lookup_node(&self, node_id: Option<PeerId>) -> Result<Vec<NodeRecord>, Discv4Error> {
        let (tx, rx) = oneshot::channel();
        let cmd = Discv4Command::Lookup { node_id, tx: Some(tx) };
        self.to_service.send(cmd).await?;
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
        // we want this message to arrive, so we clone the sender
        let _ = self.to_service.clone().try_send(cmd);
    }

    /// Adds the peer and id to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban(&self, node_id: PeerId, ip: IpAddr) {
        let cmd = Discv4Command::Ban(node_id, ip);
        // we want this message to arrive, so we clone the sender
        let _ = self.to_service.clone().try_send(cmd);
    }
    /// Adds the ip to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban_ip(&self, ip: IpAddr) {
        let cmd = Discv4Command::BanIp(ip);
        // we want this message to arrive, so we clone the sender
        let _ = self.to_service.clone().try_send(cmd);
    }

    /// Adds the peer to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    pub fn ban_node(&self, node_id: PeerId) {
        let cmd = Discv4Command::BanPeer(node_id);
        // we want this message to arrive, so we clone the sender
        let _ = self.to_service.clone().try_send(cmd);
    }

    fn send_to_service(&self, cmd: Discv4Command) {
        let _ = self.to_service.try_send(cmd).map_err(|err| {
            warn!(
                target : "discv4",
                %err,
                "dropping command",
            )
        });
    }

    /// Returns the receiver half of new listener channel that streams [`DiscoveryUpdate`]s.
    pub async fn update_stream(&self) -> Result<ReceiverStream<DiscoveryUpdate>, Discv4Error> {
        let (tx, rx) = oneshot::channel();
        let cmd = Discv4Command::Updates(tx);
        self.to_service.send(cmd).await?;
        Ok(rx.await?)
    }
}

/// Manages discv4 peer discovery over UDP.
#[must_use = "Stream does nothing unless polled"]
pub struct Discv4Service {
    /// Local address of the UDP socket.
    local_address: SocketAddr,
    /// Local ENR of the server.
    local_enr: NodeRecord,
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
    /// Whether to respect timestamps
    check_timestamps: bool,
    /// Receiver for incoming messages
    ingress: IngressReceiver,
    /// Sender for sending outgoing messages
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
    /// Currently active FindNode requests
    pending_find_nodes: HashMap<PeerId, FindNodeRequest>,
    /// Commands listener
    commands_rx: Option<mpsc::Receiver<Discv4Command>>,
    /// All subscribers for table updates
    update_listeners: Vec<mpsc::Sender<DiscoveryUpdate>>,
    /// The interval when to trigger lookups
    lookup_interval: Interval,
    /// Used to rotate targets to lookup
    lookup_rotator: LookupTargetRotator,
    /// Interval when to recheck active requests
    evict_expired_requests_interval: Interval,
    /// Interval when to resend pings.
    ping_interval: Interval,
    /// How this services is configured
    config: Discv4Config,
}

impl Discv4Service {
    /// Create a new instance for a bound [`UdpSocket`].
    pub(crate) fn new(
        socket: UdpSocket,
        local_address: SocketAddr,
        local_enr: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
        commands_rx: Option<mpsc::Receiver<Discv4Command>>,
    ) -> Self {
        // Heuristic limit for channel buffer size, which is correlated with the number of
        // concurrent requests and bucket size. This should be large enough to cover multiple
        // lookups while also anticipating incoming requests.
        const UDP_CHANNEL_BUFFER: usize = MAX_NODES_PER_BUCKET * ALPHA * (ALPHA * 2);
        let socket = Arc::new(socket);
        let (ingress_tx, ingress_rx) = mpsc::channel(UDP_CHANNEL_BUFFER);
        let (egress_tx, egress_rx) = mpsc::channel(UDP_CHANNEL_BUFFER);
        let mut tasks = JoinSet::<()>::new();

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { receive_loop(udp, ingress_tx, local_enr.id).await });

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { send_loop(udp, egress_rx).await });

        let kbuckets = KBucketsTable::new(
            local_enr.key(),
            Duration::from_secs(60),
            MAX_NODES_PER_BUCKET,
            None,
            None,
        );

        let self_lookup_interval = tokio::time::interval(config.lookup_interval);

        let ping_interval = tokio::time::interval(config.ping_interval);

        let evict_expired_requests_interval = tokio::time::interval(config.find_node_timeout);

        let lookup_rotator = if config.enable_dht_random_walk {
            LookupTargetRotator::default()
        } else {
            LookupTargetRotator::local_only()
        };

        Discv4Service {
            local_address,
            local_enr,
            _socket: socket,
            kbuckets,
            secret_key,
            _tasks: tasks,
            ingress: ingress_rx,
            egress: egress_tx,
            queued_pings: Default::default(),
            pending_pings: Default::default(),
            pending_find_nodes: Default::default(),
            check_timestamps: false,
            commands_rx,
            update_listeners: Vec::with_capacity(1),
            lookup_interval: self_lookup_interval,
            ping_interval,
            evict_expired_requests_interval,
            config,
            lookup_rotator,
        }
    }

    /// Returns the address of the UDP socket
    pub fn local_addr(&self) -> SocketAddr {
        self.local_address
    }

    /// Returns the ENR of this service.
    pub fn local_enr(&self) -> NodeRecord {
        self.local_enr
    }

    /// Returns mutable reference to ENR for testing.
    #[cfg(test)]
    pub fn local_enr_mut(&mut self) -> &mut NodeRecord {
        &mut self.local_enr
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
    /// If adding the configured boodnodes should result in a [`DiscoveryUpdate::Added`], see
    /// [`Self::add_all_nodes`].
    ///
    /// **Note:** This is a noop if there are no bootnodes.
    pub fn bootstrap(&mut self) {
        for record in self.config.bootstrap_nodes.clone() {
            debug!(target : "discv4",  ?record, "Adding bootstrap node");
            let key = kad_key(record.id);
            let entry = NodeEntry { record, last_seen: Instant::now() };

            // insert the boot node in the table
            let _ = self.kbuckets.insert_or_update(
                &key,
                entry,
                NodeStatus {
                    state: ConnectionState::Connected,
                    direction: ConnectionDirection::Outgoing,
                },
            );

            self.try_ping(record, PingReason::Normal);
        }
    }

    /// Spawns this services onto a new task
    ///
    /// Note: requires a running runtime
    pub fn spawn(mut self) -> JoinHandle<()> {
        tokio::task::spawn(async move {
            self.bootstrap();
            while let Some(event) = self.next().await {
                trace!(target : "discv4", ?event,  "processed");
            }
        })
    }

    /// Creates a new channel for [`DiscoveryUpdate`]s
    pub fn update_stream(&mut self) -> ReceiverStream<DiscoveryUpdate> {
        let (tx, rx) = mpsc::channel(512);
        self.update_listeners.push(tx);
        ReceiverStream::new(rx)
    }

    /// Looks up the local node in the DHT.
    pub fn lookup_self(&mut self) {
        self.lookup(self.local_enr.id)
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
        trace!(target : "net::discv4", ?target, "Starting lookup");
        let key = kad_key(target);

        // Start a lookup context with the 16 (MAX_NODES_PER_BUCKET) closest nodes
        let ctx = LookupContext::new(
            target,
            self.kbuckets
                .closest_values(&key)
                .take(MAX_NODES_PER_BUCKET)
                .map(|n| (key.distance(&n.key), n.value.record)),
            tx,
        );

        // From those 16, pick the 3 closest to start the lookup.
        let closest = ctx.closest(ALPHA);

        trace!(target : "net::discv4", ?target, num = closest.len(), "Start lookup closest nodes");

        for node in closest {
            self.find_node(&node, ctx.clone());
        }
    }

    /// Sends a new `FindNode` packet to the node with `target` as the lookup target.
    fn find_node(&mut self, node: &NodeRecord, ctx: LookupContext) {
        trace!(target : "discv4", ?node, lookup=?ctx.target(),  "Sending FindNode");
        ctx.mark_queried(node.id);
        let id = ctx.target();
        let msg = Message::FindNode(FindNode { id, expire: self.find_node_timeout() });
        self.send_packet(msg, node.udp_addr());
        self.pending_find_nodes.insert(node.id, FindNodeRequest::new(ctx));
    }

    /// Gets the number of entries that are considered connected.
    pub fn num_connected(&self) -> usize {
        self.kbuckets.buckets_iter().fold(0, |count, bucket| count + bucket.num_connected())
    }

    /// Notifies all listeners
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
            self.notify(DiscoveryUpdate::Removed(node_id));
        }
        removed
    }

    /// Updates the node entry
    fn update_node(&mut self, record: NodeRecord) {
        if record.id == self.local_enr.id {
            return
        }
        let key = kad_key(record.id);
        let entry = NodeEntry { record, last_seen: Instant::now() };
        match self.kbuckets.insert_or_update(
            &key,
            entry,
            NodeStatus {
                state: ConnectionState::Connected,
                direction: ConnectionDirection::Outgoing,
            },
        ) {
            InsertResult::Inserted => {
                debug!(target : "discv4",?record, "inserted new record to table");
                self.notify(DiscoveryUpdate::Added(record));
            }
            InsertResult::ValueUpdated { .. } | InsertResult::Updated { .. } => {
                trace!(target : "discv4",?record,  "updated record");
            }
            InsertResult::Failed(FailureReason::BucketFull) => {
                debug!(target : "discv4", ?record,  "discovered new record but bucket is full");
                self.notify(DiscoveryUpdate::Discovered(record));
            }
            res => {
                warn!(target : "discv4",?record, ?res,  "failed to insert");
            }
        }
    }

    /// Adds all nodes
    ///
    /// See [Self::add_node]
    pub fn add_all_nodes(&mut self, records: impl IntoIterator<Item = NodeRecord>) {
        for record in records.into_iter() {
            self.add_node(record);
        }
    }

    /// If the node's not in the table yet, this will start a ping to get it added on ping.
    pub fn add_node(&mut self, record: NodeRecord) {
        let key = kad_key(record.id);

        #[allow(clippy::single_match)]
        match self.kbuckets.entry(&key) {
            BucketEntry::Absent(_) => self.try_ping(record, PingReason::Normal),
            _ => {
                // is already in the table
            }
        }
    }

    /// Encodes the packet, sends it and returns the hash.
    pub(crate) fn send_packet(&mut self, msg: Message, to: SocketAddr) -> H256 {
        let (payload, hash) = msg.encode(&self.secret_key);
        trace!(target : "discv4",  r#type=?msg.msg_type(), ?to, ?hash, "sending packet");
        let _ = self.egress.try_send((payload, to)).map_err(|err| {
            warn!(
                target : "discv4",
                %err,
                "drop outgoing packet",
            );
        });
        hash
    }

    /// Message handler for an incoming `Ping`
    fn on_ping(&mut self, ping: Ping, remote_addr: SocketAddr, remote_id: PeerId, hash: H256) {
        // update the record
        let record = NodeRecord {
            address: ping.from.address,
            tcp_port: ping.from.tcp_port,
            udp_port: ping.from.udp_port,
            id: remote_id,
        };

        self.add_node(record);

        // send the pong
        let msg = Message::Pong(Pong { to: ping.from, echo: hash, expire: ping.expire });
        self.send_packet(msg, remote_addr);
    }

    // Guarding function for [`Self::send_ping`] that applies pre-checks
    fn try_ping(&mut self, node: NodeRecord, reason: PingReason) {
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
    pub(crate) fn send_ping(&mut self, node: NodeRecord, reason: PingReason) -> H256 {
        let remote_addr = node.udp_addr();
        let id = node.id;
        let ping =
            Ping { from: self.local_enr.into(), to: node.into(), expire: self.ping_timeout() };
        trace!(target : "discv4",  ?ping, "sending ping");
        let echo_hash = self.send_packet(Message::Ping(ping), remote_addr);

        self.pending_pings
            .insert(id, PingRequest { sent_at: Instant::now(), node, echo_hash, reason });
        echo_hash
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
                        debug!( target : "discv4",  from=?remote_addr, expected=?request.echo_hash, echo_hash=?pong.echo,"Got unexpected Pong");
                        return
                    }
                }
                entry.remove()
            }
            Entry::Vacant(_) => return,
        };

        match reason {
            PingReason::Normal => {
                self.update_node(node);
            }
            PingReason::FindNode(target, status) => {
                // update the status of the node
                match status {
                    NodeEntryStatus::Expired | NodeEntryStatus::Valid => {
                        // update node in the table
                        self.update_node(node)
                    }
                    NodeEntryStatus::IsLocal | NodeEntryStatus::Unknown => {}
                }
                // Received a pong for a discovery request
                self.respond_closest(target, remote_addr);
            }
            PingReason::Lookup(node, ctx) => {
                self.update_node(node);
                self.find_node(&node, ctx);
            }
        }
    }

    /// Handler for incoming `FindNode` message
    fn on_find_node(&mut self, msg: FindNode, remote_addr: SocketAddr, node_id: PeerId) {
        match self.node_status(node_id, remote_addr) {
            NodeEntryStatus::IsLocal => {
                // received address from self
            }
            NodeEntryStatus::Valid => self.respond_closest(msg.id, remote_addr),
            status => {
                // try to ping again
                let node = NodeRecord {
                    address: remote_addr.ip(),
                    tcp_port: remote_addr.port(),
                    udp_port: remote_addr.port(),
                    id: node_id,
                };
                self.try_ping(node, PingReason::FindNode(msg.id, status))
            }
        }
    }

    /// Handler for incoming `Neighbours` messages that are handled if they're responses to
    /// `FindNode` requests
    fn on_neighbours(&mut self, msg: Neighbours, remote_addr: SocketAddr, node_id: PeerId) {
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
                        debug!(target : "discv4", total, from=?remote_addr,  "Got oversized Neighbors packet");
                        return
                    }
                };

                if entry.get().response_count == MAX_NODES_PER_BUCKET {
                    let ctx = entry.remove().lookup_context;
                    ctx.mark_responded(node_id);
                    ctx
                } else {
                    entry.get().lookup_context.clone()
                }
            }
            Entry::Vacant(_) => {
                debug!( target : "discv4", from=?remote_addr, "Received unsolicited Neighbours");
                return
            }
        };

        let our_key = kad_key(self.local_enr.id);

        // This is the recursive lookup step where we initiate new FindNode requests for new nodes
        // that where discovered.
        for node in msg.nodes {
            // prevent banned peers from being added to the context
            if self.config.ban_list.is_banned(&node.id, &node.address) {
                trace!(target: "discv4", peer_id=?node.id, ip=?node.address, "ignoring banned record");
                continue
            }

            let key = kad_key(node.id);
            let distance = our_key.distance(&key);
            ctx.add_node(distance, node);
        }

        // get the next closest nodes, not yet queried nodes and start over.
        let closest = ctx.closest(ALPHA);

        for closest in closest {
            let key = kad_key(closest.id);
            match self.kbuckets.entry(&key) {
                BucketEntry::Absent(_) => {
                    self.try_ping(closest, PingReason::Lookup(closest, ctx.clone()))
                }
                _ => self.find_node(&closest, ctx.clone()),
            }
        }
    }

    /// Sends a Neighbours packet for `target` to the given addr
    fn respond_closest(&mut self, target: PeerId, to: SocketAddr) {
        let key = kad_key(target);
        let expire = self.send_neighbours_timeout();
        let all_nodes = self.kbuckets.closest_values(&key).collect::<Vec<_>>();

        for nodes in all_nodes.chunks(SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS) {
            let nodes = nodes.iter().map(|node| node.value.record).collect::<Vec<NodeRecord>>();
            trace!( target : "discv4",  len = nodes.len(), to=?to,"Sent neighbours packet");
            let msg = Message::Neighbours(Neighbours { nodes, expire });
            self.send_packet(msg, to);
        }
    }

    /// Returns the current status of the node
    fn node_status(&mut self, node: PeerId, addr: SocketAddr) -> NodeEntryStatus {
        if node == self.local_enr.id {
            debug!( target : "discv4",  ?node,"Got an incoming discovery request from self");
            return NodeEntryStatus::IsLocal
        }
        let key = kad_key(node);

        // Determines the status of the node based on the given address and the last observed
        // timestamp

        if let Some(node) = self.kbuckets.get_bucket(&key).and_then(|bucket| bucket.get(&key)) {
            match (node.value.is_expired(), node.value.record.udp_addr() == addr) {
                (false, true) => {
                    // valid node
                    NodeEntryStatus::Valid
                }
                (true, true) => {
                    // expired
                    NodeEntryStatus::Expired
                }
                _ => NodeEntryStatus::Unknown,
            }
        } else {
            NodeEntryStatus::Unknown
        }
    }

    fn evict_expired_requests(&mut self, now: Instant) {
        let mut nodes_to_expire = Vec::new();
        self.pending_pings.retain(|node_id, ping_request| {
            if now.duration_since(ping_request.sent_at) > self.config.ping_timeout {
                nodes_to_expire.push(*node_id);
                return false
            }
            true
        });
        self.pending_find_nodes.retain(|node_id, find_node_request| {
            if now.duration_since(find_node_request.sent_at) > self.config.find_node_timeout {
                if !find_node_request.answered {
                    nodes_to_expire.push(*node_id);
                }
                return false
            }
            true
        });
        for node_id in nodes_to_expire {
            self.expire_node_request(node_id);
        }
    }

    /// Send some pings
    fn reping_oldest(&mut self) {
        let mut nodes = self.kbuckets.iter_ref().map(|n| n.node.value).collect::<Vec<_>>();
        nodes.sort_by(|a, b| a.last_seen.cmp(&b.last_seen));
        let to_ping = nodes.into_iter().map(|n| n.record).take(MAX_NODES_PING).collect::<Vec<_>>();
        for node in to_ping {
            self.try_ping(node, PingReason::Normal)
        }
    }

    /// Removes the node from the table
    fn expire_node_request(&mut self, node_id: PeerId) {
        let key = kad_key(node_id);
        self.kbuckets.remove(&key);
    }

    /// Returns true if the expiration timestamp is considered invalid.
    fn is_expired(&self, expiration: u64) -> bool {
        self.ensure_timestamp(expiration).is_err()
    }

    /// Validate that given timestamp is not expired.
    fn ensure_timestamp(&self, expiration: u64) -> Result<(), ()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if self.check_timestamps && expiration < now {
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

    fn ping_timeout(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + self.config.ping_timeout).as_secs()
    }

    fn find_node_timeout(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + self.config.find_node_timeout)
            .as_secs()
    }

    fn send_neighbours_timeout(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + self.config.neighbours_timeout)
            .as_secs()
    }

    /// Polls the socket and advances the state.
    ///
    /// To prevent traffic amplification attacks, implementations must verify that the sender of a
    /// query participates in the discovery protocol. The sender of a packet is considered verified
    /// if it has sent a valid Pong response with matching ping hash within the last 12 hours.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Discv4Event> {
        // trigger self lookup
        if self.config.enable_lookup && self.lookup_interval.poll_tick(cx).is_ready() {
            let target = self.lookup_rotator.next(&self.local_enr.id);
            self.lookup_with(target, None);
        }

        // evict expired nodes
        if self.evict_expired_requests_interval.poll_tick(cx).is_ready() {
            self.evict_expired_requests(Instant::now())
        }

        // re-ping some peers
        if self.ping_interval.poll_tick(cx).is_ready() {
            self.reping_oldest();
        }

        // process all incoming commands
        if let Some(mut rx) = self.commands_rx.take() {
            let mut is_done = false;
            while let Poll::Ready(cmd) = rx.poll_recv(cx) {
                if let Some(cmd) = cmd {
                    match cmd {
                        Discv4Command::Lookup { node_id, tx } => {
                            let node_id = node_id.unwrap_or(self.local_enr.id);
                            self.lookup_with(node_id, tx);
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
                    }
                } else {
                    is_done = true;
                    break
                }
            }
            if !is_done {
                self.commands_rx = Some(rx);
            }
        }

        // process all incoming datagrams
        while let Poll::Ready(Some(event)) = self.ingress.poll_recv(cx) {
            match event {
                IngressEvent::RecvError(_) => {}
                IngressEvent::BadPacket(from, err, data) => {
                    warn!(target : "discv4", ?from, ?err, packet=?hex::encode(&data),   "bad packet");
                }
                IngressEvent::Packet(remote_addr, Packet { msg, node_id, hash }) => {
                    trace!( target : "discv4",  r#type=?msg.msg_type(), from=?remote_addr,"received packet");
                    return match msg {
                        Message::Ping(ping) => {
                            self.on_ping(ping, remote_addr, node_id, hash);
                            Poll::Ready(Discv4Event::Ping)
                        }
                        Message::Pong(pong) => {
                            self.on_pong(pong, remote_addr, node_id);
                            Poll::Ready(Discv4Event::Pong)
                        }
                        Message::FindNode(msg) => {
                            self.on_find_node(msg, remote_addr, node_id);
                            Poll::Ready(Discv4Event::FindNode)
                        }
                        Message::Neighbours(msg) => {
                            self.on_neighbours(msg, remote_addr, node_id);
                            Poll::Ready(Discv4Event::Neighbours)
                        }
                    }
                }
            }
        }

        // try resending buffered pings
        self.ping_buffered();

        Poll::Pending
    }
}

/// Endless future impl
impl Stream for Discv4Service {
    type Item = Discv4Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(ready!(self.get_mut().poll(cx))))
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
}

/// Continuously reads new messages from the channel and writes them to the socket
pub(crate) async fn send_loop(udp: Arc<UdpSocket>, rx: EgressReceiver) {
    let mut stream = ReceiverStream::new(rx);
    while let Some((payload, to)) = stream.next().await {
        match udp.send_to(&payload, to).await {
            Ok(size) => {
                trace!( target : "discv4",  ?to, ?size,"sent payload");
            }
            Err(err) => {
                warn!( target : "discv4",  ?to, ?err,"Failed to send datagram.");
            }
        }
    }
}

/// Continuously awaits new incoming messages and sends them back through the channel.
pub(crate) async fn receive_loop(udp: Arc<UdpSocket>, tx: IngressSender, local_id: PeerId) {
    let send = |event: IngressEvent| async {
        let _ = tx.send(event).await.map_err(|err| {
            warn!(
                target : "discv4",
                 %err,
                "failed send incoming packet",
            )
        });
    };

    loop {
        let mut buf = [0; MAX_PACKET_SIZE];
        let res = udp.recv_from(&mut buf).await;
        match res {
            Err(err) => {
                warn!(target : "discv4",  ?err, "Failed to read datagram.");
                send(IngressEvent::RecvError(err)).await;
            }
            Ok((read, remote_addr)) => {
                let packet = &buf[..read];
                match Message::decode(packet) {
                    Ok(packet) => {
                        if packet.node_id == local_id {
                            // received our own message
                            warn!(target : "discv4", ?remote_addr,  "Received own packet.");
                            continue
                        }
                        send(IngressEvent::Packet(remote_addr, packet)).await;
                    }
                    Err(err) => {
                        warn!( target : "discv4",  ?err,"Failed to decode packet");
                        send(IngressEvent::BadPacket(remote_addr, err, packet.to_vec())).await
                    }
                }
            }
        }
    }
}

/// The commands sent from the frontend to the service
enum Discv4Command {
    Ban(PeerId, IpAddr),
    BanPeer(PeerId),
    BanIp(IpAddr),
    Remove(PeerId),
    Lookup { node_id: Option<PeerId>, tx: Option<NodeRecordSender> },
    Updates(OneshotSender<ReceiverStream<DiscoveryUpdate>>),
}

/// Event type receiver produces
pub(crate) enum IngressEvent {
    /// Encountered an error when reading a datagram message.
    RecvError(io::Error),
    /// Received a bad message
    BadPacket(SocketAddr, DecodePacketError, Vec<u8>),
    /// Received a datagram from an address.
    Packet(SocketAddr, Packet),
}

/// Tracks a sent ping
struct PingRequest {
    // Timestamp when the request was sent.
    sent_at: Instant,
    // Node to which the request was sent.
    node: NodeRecord,
    // Hash sent in the Ping request
    echo_hash: H256,
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
/// If this type is dropped by all
#[derive(Clone)]
struct LookupContext {
    inner: Rc<LookupContextInner>,
}

impl LookupContext {
    /// Create new context for a recursive lookup
    fn new(
        target: PeerId,
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
        self.inner.target
    }

    /// Returns an iterator over the closest nodes.
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

    /// Inserts the node if it's missing
    fn add_node(&self, distance: Distance, record: NodeRecord) {
        let mut closest = self.inner.closest_nodes.borrow_mut();
        if let btree_map::Entry::Vacant(entry) = closest.entry(distance) {
            entry.insert(QueryNode { record, queried: false, responded: false });
        }
    }

    /// Marks the node as queried
    fn mark_queried(&self, id: PeerId) {
        if let Some((_, node)) =
            self.inner.closest_nodes.borrow_mut().iter_mut().find(|(_, node)| node.record.id == id)
        {
            node.queried = true;
        }
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

struct LookupContextInner {
    target: PeerId,
    /// The closest nodes
    closest_nodes: RefCell<BTreeMap<Distance, QueryNode>>,
    /// A listener for all the nodes retrieved in this lookup
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

/// Stored node info.
#[derive(Debug, Clone, Eq, PartialEq)]
struct NodeEntry {
    /// Node record info.
    record: NodeRecord,
    /// Timestamp of last pong.
    last_seen: Instant,
}

/// The status ofs a specific node
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeEntryStatus {
    /// Node is the local node, ourselves
    IsLocal,
    /// Node is missing in the table
    Unknown,
    /// Expired node, last seen timestamp is too long in the past
    Expired,
    /// Valid node, ready to interact with
    Valid,
}

// === impl NodeEntry ===

impl NodeEntry {
    /// Returns true if the node is considered expired.
    fn is_expired(&self) -> bool {
        self.last_seen.elapsed() > NODE_LAST_SEEN_TIMEOUT
    }
}

/// Represents why a ping is issued
enum PingReason {
    /// Standard ping
    Normal,
    /// Ping issued to adhere to endpoint proof procedure
    ///
    /// Once the expected PONG is received, the endpoint proof is complete and the find node can be
    /// answered.
    FindNode(PeerId, NodeEntryStatus),
    /// Part of a lookup to ensure endpoint is proven.
    Lookup(NodeRecord, LookupContext),
}

/// Represents node related updates state changes in the underlying node table
#[derive(Debug, Clone)]
pub enum DiscoveryUpdate {
    /// A new node was discovered _and_ added to the table.
    Added(NodeRecord),
    /// A new node was discovered but _not_ added to the table because the bucket is full.
    Discovered(NodeRecord),
    /// Node that was removed from the table
    Removed(PeerId),
    /// A series of updates
    Batch(Vec<DiscoveryUpdate>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bootnodes::mainnet_nodes,
        mock::{create_discv4, create_discv4_with_config},
    };

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

        for idx in 0..MAX_NODES_PING {
            let node = NodeRecord::new(local_addr, PeerId::random());
            service.add_node(node);
            assert!(service.pending_pings.contains_key(&node.id));
            assert_eq!(service.pending_pings.len(), idx + 1);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_lookup() {
        reth_tracing::init_tracing();

        let all_nodes = mainnet_nodes();
        let config = Discv4Config::builder().add_boot_nodes(all_nodes).build();
        let (_discv4, mut service) = create_discv4_with_config(config).await;

        let mut updates = service.update_stream();

        let _handle = service.spawn();

        let mut table = HashMap::new();
        while let Some(update) = updates.next().await {
            match update {
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_service_commands() {
        reth_tracing::init_tracing();

        let config = Discv4Config::builder().build();
        let (discv4, mut service) = create_discv4_with_config(config).await;

        service.lookup_self();

        let _handle = service.spawn();
        discv4.send_lookup_self();
        let _ = discv4.lookup_self().await;
    }
}
