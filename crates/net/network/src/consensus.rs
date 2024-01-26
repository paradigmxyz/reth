use crate::{
    cache::LruCache,
    manager::NetworkEvent,
    message::{PeerRequest, PeerRequestSender},
    metrics::{TransactionsManagerMetrics, NETWORK_POOL_TRANSACTIONS_SCOPE},
    NetworkEvents, NetworkHandle,
};
use futures::{stream::FuturesUnordered, Future, FutureExt, StreamExt};
use reth_eth_wire::{ClayerConsensusMsg, EthVersion};
use reth_interfaces::clayer::ClayerConsensus;
use reth_metrics::common::mpsc::UnboundedMeteredReceiver;
use reth_network_api::{Peers, ReputationChangeKind};
use reth_rpc_types::PeerId;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, mpsc::error::TrySendError, oneshot, oneshot::error::RecvError};
use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};
use tracing::{debug, info};

/// Manages consensus on top of the p2p network.
#[derive(Debug)]
pub struct NetworkClayerManager<Consensus> {
    /// Consensus layer.
    clayer: Consensus,
    /// Network access.
    network: NetworkHandle,
    /// From which we get all new incoming transaction related messages.
    network_events: UnboundedReceiverStream<NetworkEvent>,
    /// All the connected peers.
    peers: HashMap<PeerId, ConsensusPeer>,
    /// Incoming events from the [`NetworkManager`](crate::NetworkManager).
    consensus_events: UnboundedReceiverStream<NetworkConsensusEvent>,
    /// Incoming commands from [`ConsensussHandle`].
    pending_consensuses: ReceiverStream<(Vec<PeerId>, reth_primitives::Bytes)>,
}

impl<Consensus: ClayerConsensus> NetworkClayerManager<Consensus> {
    /// Sets up a new instance.
    ///
    /// Note: This expects an existing [`NetworkManager`](crate::NetworkManager) instance.
    pub fn new(
        network: NetworkHandle,
        clayer: Consensus,
        from_network: mpsc::UnboundedReceiver<NetworkConsensusEvent>,
    ) -> Self {
        let network_events = network.event_listener();

        // install a listener for new pending consensus that are allowed to be propagated over
        // the network
        let pending = clayer.pending_consensus_listener();

        Self {
            clayer,
            network,
            network_events,
            peers: Default::default(),
            consensus_events: UnboundedReceiverStream::new(from_network),
            pending_consensuses: ReceiverStream::new(pending),
        }
    }
}

impl<Consensus> NetworkClayerManager<Consensus>
where
    Consensus: ClayerConsensus + 'static,
{
    fn on_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::SessionClosed { peer_id, .. } => {
                // remove the peer
                self.peers.remove(&peer_id);
                self.clayer.push_network_event(peer_id, false);
            }
            NetworkEvent::SessionEstablished {
                peer_id, client_version, messages, version, ..
            } => {
                // insert a new peer into the peerset
                self.peers.insert(
                    peer_id,
                    ConsensusPeer { request_tx: messages, version, client_version },
                );
                self.clayer.push_network_event(peer_id, true);
            }
            _ => {}
        }
    }

    fn on_network_consensus_event(&mut self, event: NetworkConsensusEvent) {
        match event {
            NetworkConsensusEvent::IncomingConsensus { peer_id, msg } => {
                debug!(target: "net::consensus", ?peer_id, "received consensus broadcast");
                self.clayer.push_received_cache(peer_id, msg.0.clone());
            }
        }
    }

    fn propagate_consensus(&mut self, peers: Vec<PeerId>, data: reth_primitives::Bytes) {
        for (peer_idx, (peer_id, peer)) in self.peers.iter_mut().enumerate() {
            if peers.is_empty() {
                self.network.send_consensus(*peer_id, data.clone());
            } else {
                if peers.contains(peer_id) {
                    self.network.send_consensus(*peer_id, data.clone());
                }
            }
        }
    }
}

/// An endless future.
///
/// This should be spawned or used as part of `tokio::select!`.
impl<Consensus> Future for NetworkClayerManager<Consensus>
where
    Consensus: ClayerConsensus + Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // drain network/peer related events
        while let Poll::Ready(Some(event)) = this.network_events.poll_next_unpin(cx) {
            this.on_network_event(event);
        }

        // drain incoming transaction events
        while let Poll::Ready(Some(event)) = this.consensus_events.poll_next_unpin(cx) {
            this.on_network_consensus_event(event);
        }

        while let Poll::Ready(Some((peers, data))) = this.pending_consensuses.poll_next_unpin(cx) {
            this.propagate_consensus(peers, data);
        }

        Poll::Pending
    }
}

/// All events related to cconsensus emitted by the network.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum NetworkConsensusEvent {
    /// Received list of cconsensus from the given peer.
    ///
    /// This represents cconsensus that were broadcasted to use from the peer.
    IncomingConsensus { peer_id: PeerId, msg: ClayerConsensusMsg },
}

/// Tracks a single peer
#[derive(Debug)]
struct ConsensusPeer {
    /// A communication channel directly to the peer's session task.
    request_tx: PeerRequestSender,
    /// negotiated version of the session.
    version: EthVersion,
    /// The peer's client version.
    #[allow(unused)]
    client_version: Arc<str>,
}
