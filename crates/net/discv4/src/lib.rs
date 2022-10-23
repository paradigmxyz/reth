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
    kbucket::{InsertResult, KBucketsTable, NodeStatus},
    ConnectionDirection, ConnectionState,
};
use reth_primitives::{H256, H512};
use secp256k1::SecretKey;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    io,
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::{
        mpsc,
        oneshot::{self, Receiver as OneshotReceiver, Sender as OneshotSender},
    },
    task::JoinSet,
};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::{debug, trace, warn};

mod config;
pub mod error;
mod node;
mod proto;
use crate::{
    error::DecodePacketError,
    node::{kad_key, NodeKey, NodeRecord},
    proto::{
        ping_expiry, send_neighbours_expiry, FindNode, Message, Neighbours, Packet,
        Ping, Pong,
    },
};
pub use config::Discv4Config;
use proto::RequestId;

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

type EgressSender = mpsc::Sender<(Bytes, SocketAddr)>;
type EgressReceiver = mpsc::Receiver<(Bytes, SocketAddr)>;

type IngressSender = mpsc::Sender<IngressEvent>;
type IngressReceiver = mpsc::Receiver<IngressEvent>;

type NodeRecordSender = OneshotSender<Vec<NodeRecord>>;

/// Manages discv4 peer discovery over UDP.
#[must_use = "Stream does nothing unless polled"]
pub struct Discv4 {
    /// Local address of the UDP socket.
    local_address: SocketAddr,
    /// Local ENR of the server.
    local_enr: NodeRecord,
    /// The secret key used to sign payloads
    secret_key: SecretKey,
    /// The UDP socket for sending and receiving messages.
    socket: Arc<UdpSocket>,
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
    /// Currently active requests
    active_requests: HashMap<(NodeId, RequestId), NodeRecordRequest>,
    /// Currently active pings
    pending_pings: HashMap<H256, Vec<OneshotSender<()>>>,
    /// Buffered pending pings to apply backpressure.
    adding_nodes: VecDeque<(NodeRecord, PingReason)>,
    /// Currently active pings to specific nodes.
    in_flight_pings: HashMap<NodeId, PingRequest>,
    in_flight_find_nodes: HashMap<NodeId, FindNodeRequest>,
}

impl Discv4 {
    /// Create a new instance for a bound [`UdpSocket`].
    pub(crate) fn new(
        socket: UdpSocket,
        local_address: SocketAddr,
        local_enr: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
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

        Discv4 {
            local_address,
            local_enr,
            socket,
            kbuckets,
            secret_key,
            tasks,
            ingress: ingress_rx,
            egress: egress_tx,
            pending_events: Default::default(),
            active_requests: Default::default(),
            pending_pings: Default::default(),
            adding_nodes: Default::default(),
            in_flight_pings: Default::default(),
            in_flight_find_nodes: Default::default(),
            check_timestamps: false,
        }
    }

    /// Looks up the local node itself
    pub fn lookup_self(&mut self) -> OneshotReceiver<Vec<NodeRecord>> {
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
    pub fn lookup(&mut self, _node: NodeId) -> OneshotReceiver<Vec<NodeRecord>> {
        let (_tx, rx) = oneshot::channel();

        rx
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
    fn update_node(&mut self, _node: NodeRecord) {}

    fn insert_connected(&mut self, node_id: NodeId, record: NodeRecord) -> InsertResult<NodeKey> {
        let key = kad_key(node_id);
        let entry = NodeEntry { record, last_seen: Instant::now() };
        self.kbuckets.insert_or_update(
            &key,
            entry,
            NodeStatus {
                state: ConnectionState::Connected,
                direction: ConnectionDirection::Incoming,
            },
        )
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

        // TOOD(mattsse): handle result
        let _res = self.insert_connected(remote_id, record);

        // send a pong
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

        if self.adding_nodes.iter().any(|(n,_)| n.id == node.id) {
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
                self.respond_nearest(target, remote_addr);
            }
        }
    }

    /// Handler for incoming `FindNode` message
    fn on_find_node(&mut self, msg: FindNode, remote_addr: SocketAddr, node_id: NodeId) {
        match self.node_status(node_id, remote_addr) {
            NodeEntryStatus::IsLocal => {
                // received address from self
            }
            NodeEntryStatus::Valid => self.respond_nearest(msg.id, remote_addr),
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

    /// Sends a Neighbours packet for `target` to the given addr
    fn respond_nearest(&mut self, target: NodeId, to: SocketAddr) {
        let key = kad_key(target);
        // the size of the datagram is limited, so we chunk here the max number that fit in the
        // datagram is 13: (MAX_PACKET_SIZE - (header + expire + rlp overhead) /
        // rlplength(NodeRecord)
        const MAX_RECORDS: usize = 13usize;
        let expire = send_neighbours_expiry();
        let all_nodes = self.kbuckets.closest_values(&key).collect::<Vec<_>>();
        for nodes in all_nodes.chunks(MAX_RECORDS) {
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

    /// Polls the socket and advances the state.
    ///
    /// To prevent traffic amplification attacks, implementations must verify that the sender of a
    /// query participates in the discovery protocol. The sender of a packet is considered verified
    /// if it has sent a valid Pong response with matching ping hash within the last 12 hours.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        // // drain the events first
        // if let Some(event) = self.pending_events.pop_front() {
        //     return Poll::Ready(event)
        // }

        // read all messages
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
                    Message::Neighbours(_msg) => {}
                },
            }
        }

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

/// An active request waiting to be served.
struct NodeRecordRequest {
    started: Instant,
    tx: NodeRecordSender,
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

#[derive(Debug)]
struct FindNodeRequest {
    // Timestamp when the request was sent.
    sent_at: Instant,
    // Number of items sent by the node
    response_count: usize,
    // Whether the request has been answered yet.
    answered: bool,
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
