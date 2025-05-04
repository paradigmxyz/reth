//! Mock discovery support

use crate::{
    proto::{FindNode, Message, Neighbours, NodeEndpoint, Packet, Ping, Pong},
    receive_loop, send_loop, Discv4, Discv4Config, Discv4Service, EgressSender, IngressEvent,
    IngressReceiver, PeerId, SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS,
};
use alloy_primitives::{hex, B256};
use rand::{thread_rng, Rng, RngCore};
use reth_ethereum_forks::{ForkHash, ForkId};
use reth_network_peers::{pk2id, NodeRecord};
use secp256k1::{SecretKey, SECP256K1};
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_stream::{Stream, StreamExt};
use tracing::debug;

/// Mock discovery node
#[derive(Debug)]
pub struct MockDiscovery {
    local_addr: SocketAddr,
    local_enr: NodeRecord,
    secret_key: SecretKey,
    _udp: Arc<UdpSocket>,
    _tasks: JoinSet<()>,
    /// Receiver for incoming messages
    ingress: IngressReceiver,
    /// Sender for sending outgoing messages
    egress: EgressSender,
    pending_pongs: HashSet<PeerId>,
    pending_neighbours: HashMap<PeerId, Vec<NodeRecord>>,
    command_rx: mpsc::Receiver<MockCommand>,
}

impl MockDiscovery {
    /// Creates a new instance and opens a socket
    pub async fn new() -> io::Result<(Self, mpsc::Sender<MockCommand>)> {
        let mut rng = thread_rng();
        let socket = SocketAddr::from_str("0.0.0.0:0").unwrap();
        let (secret_key, pk) = SECP256K1.generate_keypair(&mut rng);
        let id = pk2id(&pk);
        let socket = Arc::new(UdpSocket::bind(socket).await?);
        let local_addr = socket.local_addr()?;
        let local_enr = NodeRecord {
            address: local_addr.ip(),
            tcp_port: local_addr.port(),
            udp_port: local_addr.port(),
            id,
        };

        let (ingress_tx, ingress_rx) = mpsc::channel(128);
        let (egress_tx, egress_rx) = mpsc::channel(128);
        let mut tasks = JoinSet::<()>::new();

        let udp = Arc::clone(&socket);
        tasks.spawn(receive_loop(udp, ingress_tx, local_enr.id));

        let udp = Arc::clone(&socket);
        tasks.spawn(send_loop(udp, egress_rx));

        let (tx, command_rx) = mpsc::channel(128);
        let this = Self {
            _tasks: tasks,
            ingress: ingress_rx,
            egress: egress_tx,
            local_addr,
            local_enr,
            secret_key,
            _udp: socket,
            pending_pongs: Default::default(),
            pending_neighbours: Default::default(),
            command_rx,
        };
        Ok((this, tx))
    }

    /// Spawn and consume the stream.
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::task::spawn(async move {
            let _: Vec<_> = self.collect().await;
        })
    }

    /// Queue a pending pong.
    pub fn queue_pong(&mut self, from: PeerId) {
        self.pending_pongs.insert(from);
    }

    /// Queue a pending Neighbours response.
    pub fn queue_neighbours(&mut self, target: PeerId, nodes: Vec<NodeRecord>) {
        self.pending_neighbours.insert(target, nodes);
    }

    /// Returns the local socket address associated with the service.
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns the local [`NodeRecord`] associated with the service.
    pub const fn local_enr(&self) -> NodeRecord {
        self.local_enr
    }

    /// Encodes the packet, sends it and returns the hash.
    fn send_packet(&self, msg: Message, to: SocketAddr) -> B256 {
        let (payload, hash) = msg.encode(&self.secret_key);
        let _ = self.egress.try_send((payload, to));
        hash
    }

    fn send_neighbours_timeout(&self) -> u64 {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + Duration::from_secs(30)).as_secs()
    }
}

impl Stream for MockDiscovery {
    type Item = MockEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // process all incoming commands
        while let Poll::Ready(maybe_cmd) = this.command_rx.poll_recv(cx) {
            let Some(cmd) = maybe_cmd else { return Poll::Ready(None) };
            match cmd {
                MockCommand::MockPong { node_id } => {
                    this.queue_pong(node_id);
                }
                MockCommand::MockNeighbours { target, nodes } => {
                    this.queue_neighbours(target, nodes);
                }
            }
        }

        while let Poll::Ready(Some(event)) = this.ingress.poll_recv(cx) {
            match event {
                IngressEvent::RecvError(_) => {}
                IngressEvent::BadPacket(from, err, data) => {
                    debug!(target: "discv4", ?from, %err, packet=?hex::encode(&data), "bad packet");
                }
                IngressEvent::Packet(remote_addr, Packet { msg, node_id, hash }) => match msg {
                    Message::Ping(ping) => {
                        if this.pending_pongs.remove(&node_id) {
                            let pong = Pong {
                                to: ping.from,
                                echo: hash,
                                expire: ping.expire,
                                enr_sq: None,
                            };
                            let msg = Message::Pong(pong.clone());
                            this.send_packet(msg, remote_addr);
                            return Poll::Ready(Some(MockEvent::Pong {
                                ping,
                                pong,
                                to: remote_addr,
                            }))
                        }
                    }
                    Message::Pong(_) | Message::Neighbours(_) => {}
                    Message::FindNode(msg) => {
                        if let Some(nodes) = this.pending_neighbours.remove(&msg.id) {
                            let msg = Message::Neighbours(Neighbours {
                                nodes: nodes.clone(),
                                expire: this.send_neighbours_timeout(),
                            });
                            this.send_packet(msg, remote_addr);
                            return Poll::Ready(Some(MockEvent::Neighbours {
                                nodes,
                                to: remote_addr,
                            }))
                        }
                    }
                    Message::EnrRequest(_) | Message::EnrResponse(_) => todo!(),
                },
            }
        }

        Poll::Pending
    }
}

/// Represents the event types produced by the mock service.
#[derive(Debug)]
pub enum MockEvent {
    /// A Pong event, consisting of the original Ping packet, the corresponding Pong packet,
    /// and the recipient's socket address.
    Pong {
        /// The original Ping packet.
        ping: Ping,
        /// The corresponding Pong packet.
        pong: Pong,
        /// The recipient's socket address.
        to: SocketAddr,
    },
    /// A Neighbours event, containing a list of node records and the recipient's socket address.
    Neighbours {
        /// The list of node records.
        nodes: Vec<NodeRecord>,
        /// The recipient's socket address.
        to: SocketAddr,
    },
}

/// Represents commands for interacting with the `MockDiscovery` service.
#[derive(Debug)]
pub enum MockCommand {
    /// A command to simulate a Pong event, including the node ID of the recipient.
    MockPong {
        /// The node ID of the recipient.
        node_id: PeerId,
    },
    /// A command to simulate a Neighbours event, including the target node ID and a list of node
    /// records.
    MockNeighbours {
        /// The target node ID.
        target: PeerId,
        /// The list of node records.
        nodes: Vec<NodeRecord>,
    },
}

/// Creates a new testing instance for [`Discv4`] and its service
pub async fn create_discv4() -> (Discv4, Discv4Service) {
    let fork_id = ForkId { hash: ForkHash(hex!("743f3d89")), next: 16191202 };
    create_discv4_with_config(Discv4Config::builder().add_eip868_pair("eth", fork_id).build()).await
}

/// Creates a new testing instance for [`Discv4`] and its service with the given config.
pub async fn create_discv4_with_config(config: Discv4Config) -> (Discv4, Discv4Service) {
    let mut rng = thread_rng();
    let socket = SocketAddr::from_str("0.0.0.0:0").unwrap();
    let (secret_key, pk) = SECP256K1.generate_keypair(&mut rng);
    let id = pk2id(&pk);
    let local_enr =
        NodeRecord { address: socket.ip(), tcp_port: socket.port(), udp_port: socket.port(), id };
    Discv4::bind(socket, local_enr, secret_key, config).await.unwrap()
}

/// Generates a random [`NodeEndpoint`] using the provided random number generator.
pub fn rng_endpoint(rng: &mut impl Rng) -> NodeEndpoint {
    let address = if rng.gen() {
        let mut ip = [0u8; 4];
        rng.fill_bytes(&mut ip);
        IpAddr::V4(ip.into())
    } else {
        let mut ip = [0u8; 16];
        rng.fill_bytes(&mut ip);
        IpAddr::V6(ip.into())
    };
    NodeEndpoint { address, tcp_port: rng.gen(), udp_port: rng.gen() }
}

/// Generates a random [`NodeRecord`] using the provided random number generator.
pub fn rng_record(rng: &mut impl RngCore) -> NodeRecord {
    let NodeEndpoint { address, udp_port, tcp_port } = rng_endpoint(rng);
    NodeRecord { address, tcp_port, udp_port, id: rng.gen() }
}

/// Generates a random IPv6 [`NodeRecord`] using the provided random number generator.
pub fn rng_ipv6_record(rng: &mut impl RngCore) -> NodeRecord {
    let mut ip = [0u8; 16];
    rng.fill_bytes(&mut ip);
    let address = IpAddr::V6(ip.into());
    NodeRecord { address, tcp_port: rng.gen(), udp_port: rng.gen(), id: rng.gen() }
}

/// Generates a random IPv4 [`NodeRecord`] using the provided random number generator.
pub fn rng_ipv4_record(rng: &mut impl RngCore) -> NodeRecord {
    let mut ip = [0u8; 4];
    rng.fill_bytes(&mut ip);
    let address = IpAddr::V4(ip.into());
    NodeRecord { address, tcp_port: rng.gen(), udp_port: rng.gen(), id: rng.gen() }
}

/// Generates a random [`Message`] using the provided random number generator.
pub fn rng_message(rng: &mut impl RngCore) -> Message {
    match rng.gen_range(1..=4) {
        1 => Message::Ping(Ping {
            from: rng_endpoint(rng),
            to: rng_endpoint(rng),
            expire: rng.gen(),
            enr_sq: None,
        }),
        2 => Message::Pong(Pong {
            to: rng_endpoint(rng),
            echo: rng.gen(),
            expire: rng.gen(),
            enr_sq: None,
        }),
        3 => Message::FindNode(FindNode { id: rng.gen(), expire: rng.gen() }),
        4 => {
            let num: usize = rng.gen_range(1..=SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS);
            Message::Neighbours(Neighbours {
                nodes: std::iter::repeat_with(|| rng_record(rng)).take(num).collect(),
                expire: rng.gen(),
            })
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Discv4Event;
    use std::net::Ipv4Addr;

    /// This test creates two local UDP sockets. The mocked discovery service responds to specific
    /// messages and we check the actual service receives answers
    #[tokio::test]
    async fn can_mock_discovery() {
        reth_tracing::init_test_tracing();

        let mut rng = thread_rng();
        let (_, mut service) = create_discv4().await;
        let (mut mockv4, _cmd) = MockDiscovery::new().await.unwrap();

        let mock_enr = mockv4.local_enr();

        // we only want to test internally
        service.local_enr_mut().address = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

        let discv_addr = service.local_addr();
        let discv_enr = service.local_enr();

        // make sure it responds with a Pong
        mockv4.queue_pong(discv_enr.id);

        // This sends a ping to the mock service
        service.add_node(mock_enr);

        // process the mock pong
        let event = mockv4.next().await.unwrap();
        match event {
            MockEvent::Pong { ping: _, pong: _, to } => {
                assert_eq!(to, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), discv_addr.port()));
            }
            MockEvent::Neighbours { .. } => {
                unreachable!("invalid response")
            }
        }

        // discovery service received mocked pong
        let event = service.next().await.unwrap();
        assert_eq!(event, Discv4Event::Pong);

        assert!(service.contains_node(mock_enr.id));

        let mock_nodes =
            std::iter::repeat_with(|| rng_record(&mut rng)).take(5).collect::<Vec<_>>();

        mockv4.queue_neighbours(discv_enr.id, mock_nodes.clone());

        // start lookup
        service.lookup_self();

        let event = mockv4.next().await.unwrap();
        match event {
            MockEvent::Pong { .. } => {
                unreachable!("invalid response")
            }
            MockEvent::Neighbours { nodes, to } => {
                assert_eq!(to, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), discv_addr.port()));
                assert_eq!(nodes, mock_nodes);
            }
        }

        // discovery service received mocked pong
        let event = service.next().await.unwrap();
        assert_eq!(event, Discv4Event::Neighbours);
    }
}
