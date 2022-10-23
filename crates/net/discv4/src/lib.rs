#![warn(missing_docs)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
// TODO remove later
#![allow(dead_code)]

//! discv4 implementation: <https://github.com/ethereum/devp2p/blob/master/discv4.md>

use bytes::Bytes;
use discv5::{
    kbucket::{Entry as BucketEntry, KBucketsTable, NodeStatus, MAX_NODES_PER_BUCKET},
    ConnectionDirection, ConnectionState,
};
use reth_primitives::{H256, H512};
use secp256k1::SecretKey;
use std::{
    cell::RefCell,
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    io,
    net::SocketAddr,
    rc::Rc,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot, oneshot::Sender as OneshotSender},
    task::JoinSet,
    time::Interval,
};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::{debug, trace, warn};

mod config;
pub mod error;
mod node;
mod proto;
use crate::{
    error::{DecodePacketError, Discv4Error},
    node::{kad_key, NodeKey, NodeRecord},
    proto::{
        find_node_expiry, ping_expiry, send_neighbours_expiry, FindNode, Message, Neighbours,
        Packet, Ping, Pong, FIND_NODE_TIMEOUT, MAX_DATAGRAM_NEIGHBOUR_RECORDS, PING_TIMEOUT,
    },
};
pub use config::Discv4Config;

/// Identifier for nodes.
pub type NodeId = H512;

/// The maximum size of any packet is 1280 bytes.
const MAX_PACKET_SIZE: usize = 1280;

/// Length of the packet-header: Hash + Signature + Packet Type
const MIN_PACKET_SIZE: usize = 32 + 65 + 1;

const ALPHA: usize = 3; // Denoted by \alpha in [Kademlia]. Number of concurrent FindNode requests.

const MAX_NODES_PING: usize = 32; // Max nodes to add/ping at once

/// The timeout used to identify expired nodes
const NODE_LAST_SEEN_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// The interval to use for rediscovery
pub const SELF_LOOKUP_INTERVAL: Duration = Duration::from_secs(60);

type EgressSender = mpsc::Sender<(Bytes, SocketAddr)>;
type EgressReceiver = mpsc::Receiver<(Bytes, SocketAddr)>;

type IngressSender = mpsc::Sender<IngressEvent>;
type IngressReceiver = mpsc::Receiver<IngressEvent>;

type NodeRecordSender = OneshotSender<Vec<NodeRecord>>;

/// The Discv4 frontend
#[derive(Debug, Clone)]
pub struct Discv4 {
    /// channel to send commands over to the service
    to_service: mpsc::Sender<Discv4Command>,
}

impl Discv4 {
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
    pub async fn lookup(&self, node_id: NodeId) -> Result<Vec<NodeRecord>, Discv4Error> {
        let (tx, rx) = oneshot::channel();
        let cmd = Discv4Command::Lookup { node_id, tx };
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
    socket: Arc<UdpSocket>,
    /// The boot node identifiers
    bootstrap_nodes: HashSet<NodeId>,
    /// The routing table.
    kbuckets: KBucketsTable<NodeKey, NodeEntry>,
    /// Whether to respect timestamps
    check_timestamps: bool,
    /// The spawned UDP tasks.
    ///
    /// Note: If dropped, the spawned tasks are aborted.
    tasks: JoinSet<()>,
    /// Receiver for incoming messages
    ingress: IngressReceiver,
    /// Sender for sending outgoing messages
    egress: EgressSender,
    /// Buffered events produced ready to return
    pending_events: VecDeque<IngressEvent>,
    /// Currently active pings
    pending_pings: HashMap<H256, Vec<OneshotSender<()>>>,
    /// Buffered pending pings to apply backpressure.
    adding_nodes: VecDeque<(NodeRecord, PingReason)>,
    /// Currently active pings to specific nodes.
    in_flight_pings: HashMap<NodeId, PingRequest>,
    /// Currently active FindNode requests
    in_flight_find_nodes: HashMap<NodeId, FindNodeRequest>,
    /// Commands listener
    commands_rx: Option<mpsc::Receiver<Discv4Command>>,
    /// The interval when to trigger self lookup
    self_lookup_interval: Interval,
    /// Interval when to recheck active requests
    evict_expired_requests_interval: Interval,
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
        let socket = Arc::new(socket);
        let (ingress_tx, ingress_rx) = mpsc::channel(1024);
        let (egress_tx, egress_rx) = mpsc::channel(1024);
        let mut tasks = JoinSet::<()>::new();

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { receive_loop(udp, ingress_tx, local_enr.id).await });

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { send_loop(udp, egress_rx).await });

        let kbuckets = KBucketsTable::new(
            local_enr.key(),
            Duration::from_secs(60),
            config.incoming_bucket_limit,
            None,
            None,
        );

        let self_lookup_interval = tokio::time::interval(SELF_LOOKUP_INTERVAL);

        let evict_expired_requests_interval = tokio::time::interval(PING_TIMEOUT);

        Discv4Service {
            local_address,
            local_enr,
            socket,
            kbuckets,
            secret_key,
            tasks,
            ingress: ingress_rx,
            egress: egress_tx,
            pending_events: Default::default(),
            pending_pings: Default::default(),
            adding_nodes: Default::default(),
            in_flight_pings: Default::default(),
            in_flight_find_nodes: Default::default(),
            check_timestamps: false,
            bootstrap_nodes: Default::default(),
            commands_rx,
            self_lookup_interval,
            evict_expired_requests_interval,
        }
    }

    /// Looks up the local node itself.
    pub fn lookup_self(&mut self) {
        self.lookup(self.local_enr.id)
    }

    /// Looks up the given node.
    ///
    /// A FindNode packet requests information about nodes close to target. The target is a 64-byte
    /// secp256k1 public key. When FindNode is received, the recipient should reply with Neighbors
    /// packets containing the closest 16 nodes to target found in its local table.
    //
    // To guard against traffic amplification attacks, Neighbors replies should only be sent if the
    // sender of FindNode has been verified by the endpoint proof procedure.
    pub fn lookup(&mut self, target: NodeId) {
        self.lookup_with(target, None)
    }

    fn lookup_with(&mut self, target: NodeId, resp: Option<NodeRecordResponse>) {
        let key = kad_key(target);
        // This contains only verified nodes
        #[allow(clippy::needless_collect)] // silence false clippy lint
        let closest = self
            .kbuckets
            .closest_values(&key)
            .filter(|n| !self.bootstrap_nodes.contains(&n.value.record.id))
            .take(ALPHA)
            .collect::<Vec<_>>();
        for (idx, node) in closest.into_iter().enumerate() {
            self.find_node(&node.value, target, resp.clone(), idx);
        }
    }

    /// Sends a new `FindNode` packet to the node with `target` as the lookup target.
    fn find_node(
        &mut self,
        node: &NodeEntry,
        target: NodeId,
        resp: Option<NodeRecordResponse>,
        concurrency_num: usize,
    ) {
        debug_assert!(concurrency_num < ALPHA, "MAX allowed concurrency is 3");
        let msg = Message::FindNode(FindNode { id: target, expire: find_node_expiry() });
        self.send_packet(msg, node.record.udp_addr());
        self.in_flight_find_nodes
            .insert(node.record.id, FindNodeRequest::new(concurrency_num, resp));
    }

    /// Gets the number of entries that are considered connected.
    pub fn num_connected(&self) -> usize {
        self.kbuckets.buckets_iter().fold(0, |count, bucket| count + bucket.num_connected())
    }

    /// Removes a `node_id` from the routing table.
    ///
    /// This allows applications, for whatever reason, to remove nodes from the local routing
    /// table. Returns `true` if the node was in the table and `false` otherwise.
    pub fn remove_node(&mut self, node_id: NodeId) -> bool {
        let key = kad_key(node_id);
        self.kbuckets.remove(&key)
    }

    /// Updates the node entry
    fn update_node(&mut self, record: NodeRecord) {
        if record.id == self.local_enr.id {
            return
        }
        let key = kad_key(record.id);
        let entry = NodeEntry { record, last_seen: Instant::now() };

        let _ = self.kbuckets.insert_or_update(
            &key,
            entry,
            NodeStatus {
                state: ConnectionState::Connected,
                direction: ConnectionDirection::Incoming,
            },
        );
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
    fn send_packet(&mut self, msg: Message, to: SocketAddr) -> H256 {
        let (payload, hash) = msg.encode(&self.secret_key);
        let _ = self.egress.try_send((payload, to));
        hash
    }

    /// Message handler for an incoming `Ping`
    fn on_ping(&mut self, ping: Ping, remote_addr: SocketAddr, remote_id: NodeId, hash: H256) {
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
        if self.in_flight_pings.contains_key(&node.id) ||
            self.in_flight_find_nodes.contains_key(&node.id)
        {
            return
        }

        if self.adding_nodes.iter().any(|(n, _)| n.id == node.id) {
            return
        }

        if self.in_flight_pings.len() < MAX_NODES_PING {
            self.send_ping(node, reason)
        } else {
            self.adding_nodes.push_back((node, reason))
        }
    }

    /// Sends a ping message to the node's UDP address.
    fn send_ping(&mut self, node: NodeRecord, reason: PingReason) {
        let remote_addr = node.udp_addr();
        let id = node.id;
        let msg = Message::Ping(Ping {
            from: self.local_enr.into(),
            to: node.into(),
            expire: ping_expiry(),
        });
        let echo_hash = self.send_packet(msg, remote_addr);

        self.in_flight_pings
            .insert(id, PingRequest { sent_at: Instant::now(), node, echo_hash, reason });
    }

    /// Message handler for an incoming `Pong`.
    fn on_pong(&mut self, pong: Pong, remote_addr: SocketAddr, remote_id: NodeId, echo_hash: H256) {
        if self.is_expired(pong.expire) {
            return
        }

        let PingRequest { node, reason, .. } = match self.in_flight_pings.entry(remote_id) {
            Entry::Occupied(entry) => {
                {
                    let request = entry.get();
                    if request.echo_hash != echo_hash {
                        debug!(from=?remote_addr, expected=?request.echo_hash, packet_hash=?echo_hash, target = "net::disc", "Got unexpected Pong");
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
        }
    }

    /// Handler for incoming `FindNode` message
    fn on_find_node(&mut self, msg: FindNode, remote_addr: SocketAddr, node_id: NodeId) {
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

    /// Handler for incoming `Neighbours` message
    fn on_neighbours(&mut self, msg: Neighbours, remote_addr: SocketAddr, node_id: NodeId) {
        // check if this request was expected
        let is_response = match self.in_flight_find_nodes.entry(node_id) {
            Entry::Occupied(mut entry) => {
                let expected = {
                    let request = entry.get_mut();
                    // Mark the request as answered
                    request.answered = true;
                    let total = request.response_count + msg.nodes.len();
                    // Neighbours response is exactly 1 bucket (16 entries).
                    if total <= MAX_NODES_PER_BUCKET {
                        request.response_count = total;

                        // If this response is being watched then we store the nodes.
                        if let Some(ref resp) = request.resp {
                            resp.extend_request(request.concurrency_num, msg.nodes.clone())
                        }

                        true
                    } else {
                        debug!(total, from=?remote_addr, target = "net::disc", "Got oversized Neighbors packet");
                        false
                    }
                };
                if entry.get().response_count == MAX_NODES_PER_BUCKET {
                    entry.remove();
                }
                expected
            }
            Entry::Vacant(_) => {
                debug!(from=?remote_addr, target = "net::disc", "Received unsolicited Neighbours");
                false
            }
        };

        if !is_response {
            return
        }

        // add all the nodes
        for record in msg.nodes {
            self.add_node(record)
        }
    }

    /// Sends a Neighbours packet for `target` to the given addr
    fn respond_closest(&mut self, target: NodeId, to: SocketAddr) {
        let key = kad_key(target);
        let expire = send_neighbours_expiry();
        let all_nodes = self.kbuckets.closest_values(&key).collect::<Vec<_>>();

        // The max datagram size allows 12 IPV6 nodes, or 11 IPV6 + 2 IPV4 nodes
        for nodes in all_nodes.chunks(MAX_DATAGRAM_NEIGHBOUR_RECORDS) {
            let nodes = nodes.iter().map(|node| node.value.record).collect::<Vec<NodeRecord>>();
            trace!(len = nodes.len(), to=?to, target = "net::disc", "Sent neighbours packet");
            let msg = Message::Neighbours(Neighbours { nodes, expire });
            self.send_packet(msg, to);
        }
    }

    /// Returns the current status of the node
    fn node_status(&mut self, node: NodeId, addr: SocketAddr) -> NodeEntryStatus {
        if node == self.local_enr.id {
            debug!(?node, target = "net::disc", "Got an incoming discovery request from self");
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
        self.in_flight_pings.retain(|node_id, ping_request| {
            if now.duration_since(ping_request.sent_at) > PING_TIMEOUT {
                nodes_to_expire.push(*node_id);
                return false
            }
            true
        });
        self.in_flight_find_nodes.retain(|node_id, find_node_request| {
            if now.duration_since(find_node_request.sent_at) > FIND_NODE_TIMEOUT {
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

    /// Removes the node from the table
    fn expire_node_request(&mut self, node_id: NodeId) {
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
            debug!(target: "net::disc", "Expired packet");
            return Err(())
        }
        Ok(())
    }

    /// Pops buffered ping requests and sends them.
    fn ping_buffered(&mut self) {
        while self.in_flight_pings.len() < MAX_NODES_PING {
            match self.adding_nodes.pop_front() {
                Some((next, reason)) => self.try_ping(next, reason),
                None => break,
            }
        }
    }

    /// Polls the socket and advances the state.
    ///
    /// To prevent traffic amplification attacks, implementations must verify that the sender of a
    /// query participates in the discovery protocol. The sender of a packet is considered verified
    /// if it has sent a valid Pong response with matching ping hash within the last 12 hours.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        // trigger self lookup
        if self.self_lookup_interval.poll_tick(cx).is_ready() {
            self.lookup_self()
        }

        // evict expired nodes
        if self.evict_expired_requests_interval.poll_tick(cx).is_ready() {
            self.evict_expired_requests(Instant::now())
        }

        // process all incoming commands
        if let Some(mut rx) = self.commands_rx.take() {
            let mut is_done = false;
            while let Poll::Ready(cmd) = rx.poll_recv(cx) {
                if let Some(cmd) = cmd {
                    match cmd {
                        Discv4Command::Lookup { node_id, tx } => {
                            self.lookup_with(node_id, Some(NodeRecordResponse::new(tx)));
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
                IngressEvent::BadPacket(_from, _err) => {}
                IngressEvent::Packet(remote_addr, Packet { msg, node_id, hash }) => match msg {
                    Message::Ping(ping) => {
                        self.on_ping(ping, remote_addr, node_id, hash);
                    }
                    Message::Pong(pong) => {
                        self.on_pong(pong, remote_addr, node_id, hash);
                    }
                    Message::FindNode(msg) => {
                        self.on_find_node(msg, remote_addr, node_id);
                    }
                    Message::Neighbours(msg) => {
                        self.on_neighbours(msg, remote_addr, node_id);
                    }
                },
            }
        }

        // try resending buffered pings
        self.ping_buffered();

        Poll::Pending
    }
}

/// Continuously reads new messages from the channel and writes them to the socket
async fn send_loop(udp: Arc<UdpSocket>, rx: EgressReceiver) {
    let mut stream = ReceiverStream::new(rx);
    while let Some((payload, to)) = stream.next().await {
        if let Err(err) = udp.send_to(&payload, to).await {
            warn!(?err, target = "net::disc", "Failed to send datagram.");
        }
    }
}

/// Continuously awaits new incoming messages and sends them back through the channel.
async fn receive_loop(udp: Arc<UdpSocket>, tx: IngressSender, local_id: NodeId) {
    loop {
        let mut buf = [0; MAX_PACKET_SIZE];
        let res = udp.recv_from(&mut buf).await;
        match res {
            Err(err) => {
                warn!(?err, target = "net::disc", "Failed to read datagram.");
                let _ = tx.send(IngressEvent::RecvError(err)).await;
            }
            Ok((read, remote_addr)) => {
                let packet = &buf[..read];
                match Message::decode(packet) {
                    Ok(packet) => {
                        if packet.node_id == local_id {
                            // received our own message
                            warn!(?remote_addr, target = "net::disc", "Received own packet.");
                            continue
                        }
                        let _ = tx.send(IngressEvent::Packet(remote_addr, packet)).await;
                    }
                    Err(err) => {
                        warn!(?err, target = "net::disc", "Failed to decode packet");
                        let _ = tx.send(IngressEvent::BadPacket(remote_addr, err)).await;
                    }
                }
            }
        }
    }
}

/// The commands sent from the frontend to the service
enum Discv4Command {
    Lookup { node_id: NodeId, tx: NodeRecordSender },
}

/// Event type receiver produces
pub(crate) enum IngressEvent {
    /// Encountered an error when reading a datagram message.
    RecvError(io::Error),
    /// Received a bad message
    BadPacket(SocketAddr, DecodePacketError),
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
    echo_hash: H256,
    /// Why this ping was sent.
    reason: PingReason,
}

/// Tracks responses for 3 concurrent `FindNode` requests.
///
/// If this type is dropped by all
#[derive(Debug, Clone)]
struct NodeRecordResponse {
    inner: Rc<NodeRecordResponseInner>,
}

impl NodeRecordResponse {
    /// Create new response buffer
    fn new(tx: NodeRecordSender) -> Self {
        let inner =
            Rc::new(NodeRecordResponseInner { buffer: Rc::new(Default::default()), tx: Some(tx) });
        Self { inner }
    }

    fn extend_request(&self, idx: usize, nodes: Vec<NodeRecord>) {
        match idx {
            1 => self.inner.buffer.first.borrow_mut().extend(nodes),
            2 => self.inner.buffer.second.borrow_mut().extend(nodes),
            3 => self.inner.buffer.third.borrow_mut().extend(nodes),
            _ => unreachable!(),
        }
    }
}

#[derive(Debug)]
struct NodeRecordResponseInner {
    buffer: Rc<ConcurrentFindNodeBuffer>,
    tx: Option<NodeRecordSender>,
}

impl Drop for NodeRecordResponseInner {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            // there's only 1 instance shared across `FindNode` requests, if this is dropped then
            // all requests finished, and we can send all results back
            let mut nodes = self.buffer.first.take();
            nodes.extend(self.buffer.second.take());
            nodes.extend(self.buffer.third.take());
            let _ = tx.send(nodes);
        }
    }
}

/// A buffer for concurrent `FindNode` requests
///
/// The concurrency parameter ALPHA allows `3` `FindNode` requests.
/// This will gather all responses from those 3 individual lookups.
#[derive(Debug)]
struct ConcurrentFindNodeBuffer {
    first: RefCell<Vec<NodeRecord>>,
    second: RefCell<Vec<NodeRecord>>,
    third: RefCell<Vec<NodeRecord>>,
}

impl Default for ConcurrentFindNodeBuffer {
    fn default() -> Self {
        Self {
            first: RefCell::new(Vec::with_capacity(MAX_NODES_PER_BUCKET)),
            second: RefCell::new(Vec::with_capacity(MAX_NODES_PER_BUCKET)),
            third: RefCell::new(Vec::with_capacity(MAX_NODES_PER_BUCKET)),
        }
    }
}

#[derive(Debug)]
struct FindNodeRequest {
    /// The concurrency identifier index in a `FindNode` query, where max is `ALPHAS`
    concurrency_num: usize,
    // Timestamp when the request was sent.
    sent_at: Instant,
    // Number of items sent by the node
    response_count: usize,
    // Whether the request has been answered yet.
    answered: bool,
    /// Response buffer
    resp: Option<NodeRecordResponse>,
}

// === impl FindNodeRequest ===

impl FindNodeRequest {
    fn new(concurrency_num: usize, resp: Option<NodeRecordResponse>) -> Self {
        Self { concurrency_num, sent_at: Instant::now(), response_count: 0, answered: false, resp }
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
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PingReason {
    /// Standard ping
    Normal,
    /// Ping issued to adhere to endpoint proof procedure
    ///
    /// Once the expected PONG is received, the endpoint proof is complete and the find node can be
    /// answered.
    FindNode(NodeId, NodeEntryStatus),
}
