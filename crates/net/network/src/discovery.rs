//! devp2p peer discovery support.
//!
//! <https://github.com/ethereum/devp2p/blob/master/discv4.md>

use pin_project::pin_project;
use reth_primitives::H256;
use std::{io, net::SocketAddr, sync::Arc};
use tokio::{net::UdpSocket, sync::mpsc, task::JoinSet};
use tracing::warn;
use reth_primitives::sha3::Keccak256;

/// The maximum size of any packet is 1280 bytes.
const MAX_PACKET_SIZE: usize = 1280;

/// Length of the packet-header: Hash + Signature + Packet Type
const MIN_PACKET_SIZE: usize = 32 + 65 + 1;

type Sender = mpsc::Sender<DiscoveryEvent>;
type Receiver = mpsc::Receiver<DiscoveryEvent>;

/// Manages discv peer discovery over UDP.
#[must_use = "Stream does nothing unless polled"]
#[pin_project]
pub struct Discovery {
    /// Local address of the UDP socket.
    local_address: SocketAddr,
    /// The UDP socket for sending and receiving messages.
    socket: Arc<UdpSocket>,
    /// The spawned UDP tasks.
    ///
    /// Note: If dropped, the spawned tasks are aborted.
    tasks: JoinSet<()>,
    /// Receiver for incoming messages
    #[pin]
    incoming: Receiver,
    /// Sender for
    outgoing: Sender,
}

impl Discovery {
    /// Create a new instance for a bound [`UdpSocket`].
    pub fn new(socket: UdpSocket, local_addr: SocketAddr) -> io::Result<Self> {
        todo!()
    }
}

/// Continuously awaits new incoming messages and sends them back through the channel.
async fn receive(udp: Arc<UdpSocket>, tx: Sender) {

    let send = async |event| {
        let _ = tx.send(event).await;
    };

    loop {
        let mut buf = [0; MAX_PACKET_SIZE];
        let res = udp.recv_from(&mut buf).await;
        match res {
            Err(err) => {
                warn!(?err, target: "net::disc", "Failed to read discovery datagram.");
                send(DiscoveryEvent::RecvError(err)).await;
            }
            Ok((read, remote_addr)) => {
                let packet = &buf[..read];

                if packet.len() < MIN_PACKET_SIZE {
                    send(DiscoveryEvent::BadPacket(remote_addr)).await;
                    continue
                }

                let hash = Keccak256(&buf[32..]);
                let check_hash = H256::from_slice(&buf[..32]);
                if check_hash != hash {
                    bail!("Hash check failed: computed {}, prefix {}", hash, check_hash);
                }
            }
        }
    }
}

/// Event type the [`Discovery`] manager produces
pub(crate) enum DiscoveryEvent {
    /// Encountered an error when reading a datagram message.
    RecvError(io::Error),
    /// Received a bad message
    BadPacket(SocketAddr),
}
