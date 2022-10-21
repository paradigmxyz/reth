#![warn(missing_docs)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
// TODO remove later
#![allow(dead_code)]

//! discv4 implementation: <https://github.com/ethereum/devp2p/blob/master/discv4.md>

use discv5::kbucket::KBucketsTable;
use reth_primitives::{keccak256, H256, H512};
use std::{
    collections::VecDeque,
    io,
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{net::UdpSocket, sync::mpsc, task::JoinSet};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::warn;

mod config;
mod node;
mod proto;
use crate::node::{NodeKey, NodeRecord};
pub use config::Discv4Config;

/// Identifier for nodes.
pub type NodeId = H512;

/// The maximum size of any packet is 1280 bytes.
const MAX_PACKET_SIZE: usize = 1280;

/// Length of the packet-header: Hash + Signature + Packet Type
const MIN_PACKET_SIZE: usize = 32 + 65 + 1;

type EgressSender = mpsc::Sender<()>;
type EgressReceiver = mpsc::Receiver<()>;

type IngressSender = mpsc::Sender<IngressEvent>;
type IngressReceiver = mpsc::Receiver<IngressEvent>;

/// Manages discv4 peer discovery over UDP.
#[must_use = "Stream does nothing unless polled"]
pub struct Discv4 {
    /// Local address of the UDP socket.
    local_address: SocketAddr,
    /// Local ENR of the server.
    local_enr: NodeRecord,
    /// The UDP socket for sending and receiving messages.
    socket: Arc<UdpSocket>,
    /// The routing table.
    kbuckets: KBucketsTable<NodeKey, NodeRecord>,
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
}

impl Discv4 {
    /// Create a new instance for a bound [`UdpSocket`].
    pub(crate) fn new(
        socket: UdpSocket,
        local_address: SocketAddr,
        local_enr: NodeRecord,
        config: Discv4Config,
    ) -> Self {
        let socket = Arc::new(socket);
        let (ingress_tx, ingress_rx) = mpsc::channel(1024);
        let (egress_tx, egress_rx) = mpsc::channel(1024);
        let mut tasks = JoinSet::<()>::new();

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { receive_loop(udp, ingress_tx).await });

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { send_loop(udp, egress_rx).await });

        let kbuckets = KBucketsTable::new(
            local_enr.key(),
            Duration::from_secs(60),
            config.incoming_bucket_limit,
            None,
            None,
        );

        let disc = Discv4 {
            local_address,
            local_enr,
            socket,
            kbuckets,
            tasks,
            ingress: ingress_rx,
            egress: egress_tx,
            pending_events: Default::default(),
        };

        disc
    }

    /// Polls the socket and advances the state.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<IngressEvent> {
        loop {
            // drain the events first
            if let Some(event) = self.pending_events.pop_front() {
                return Poll::Ready(event)
            }

            // read all messages
            while let Poll::Ready(Some(event)) = self.ingress.poll_recv(cx) {
                self.pending_events.push_back(event);
            }

            // nothing produced
            if self.pending_events.is_empty() {
                return Poll::Pending
            }
        }
    }
}

/// Continuously reads new messages from the channel and writes them to the socket
async fn send_loop(_udp: Arc<UdpSocket>, rx: EgressReceiver) {
    let mut stream = ReceiverStream::new(rx);
    while let Some(_msg) = stream.next().await {
        todo!("write to socket")
    }
}

/// Continuously awaits new incoming messages and sends them back through the channel.
async fn receive_loop(udp: Arc<UdpSocket>, tx: IngressSender) {
    loop {
        let mut buf = [0; MAX_PACKET_SIZE];
        let res = udp.recv_from(&mut buf).await;
        match res {
            Err(err) => {
                warn!(?err, target = "net::disc", "Failed to read discovery datagram.");
                let _ = tx.send(IngressEvent::RecvError(err)).await;
            }
            Ok((read, remote_addr)) => {
                let packet = &buf[..read];

                if packet.len() < MIN_PACKET_SIZE {
                    let _ = tx
                        .send(IngressEvent::BadPacket(remote_addr, BadPacketKind::InvalidFormat))
                        .await;
                    continue
                }

                // parses the wire-protocol, every packet starts with a header:
                // packet-header = hash || signature || packet-type
                // hash = keccak256(signature || packet-type || packet-data)
                // signature = sign(packet-type || packet-data)

                let header_hash = keccak256(&buf[32..]);
                let data_hash = H256::from_slice(&buf[..32]);
                if data_hash != header_hash {
                    let _ = tx
                        .send(IngressEvent::BadPacket(remote_addr, BadPacketKind::HashMismatch))
                        .await;
                    continue
                }

                // TODO recovery message parsing

                todo!("")
            }
        }
    }
}

/// Event type receiver produces
pub(crate) enum IngressEvent {
    /// Encountered an error when reading a datagram message.
    RecvError(io::Error),
    /// Received a bad message
    BadPacket(SocketAddr, BadPacketKind),
}

/// Possible reasons why receiving a packet failed
pub(crate) enum BadPacketKind {
    /// Malformed packet data
    InvalidFormat,
    /// Hash of the header not equals to the hash of the data.
    HashMismatch,
}

/// How we connected to the node.
#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum ConnectionDirection {
    /// The node contacted us.
    Incoming,
    /// We contacted the node.
    Outgoing,
}
