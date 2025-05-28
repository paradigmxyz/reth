use crate::{ZkRessMessage, ZkRessProtocolMessage, ZkRessProtocolProvider};
use alloy_consensus::Header;
use alloy_primitives::{bytes::BytesMut, BlockHash, B256};
use futures::{stream::FuturesUnordered, Stream, StreamExt};
use reth_eth_wire::{message::RequestPair, multiplex::ProtocolConnection};
use reth_ethereum_primitives::BlockBody;
use reth_network_api::{test_utils::PeersHandle, PeerId, ReputationChangeKind};
use reth_ress_protocol::{GetHeaders, NodeType};
use reth_storage_errors::ProviderResult;
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

/// The connection handler for the custom `RLPx` protocol.
#[derive(Debug)]
pub struct ZkRessProtocolConnection<P>
where
    P: ZkRessProtocolProvider,
{
    /// Provider.
    provider: P,
    /// The type of this node..
    node_type: NodeType,
    /// Peers handle.
    peers_handle: PeersHandle,
    /// Peer ID.
    peer_id: PeerId,
    /// Protocol connection.
    conn: ProtocolConnection,
    /// Stream of incoming commands.
    commands: UnboundedReceiverStream<ZkRessPeerRequest<P::Witness>>,
    /// The total number of active connections.
    active_connections: Arc<AtomicU64>,
    /// Flag indicating whether the node type was sent to the peer.
    node_type_sent: bool,
    /// Flag indicating whether this stream has previously been terminated.
    terminated: bool,
    /// Incremental counter for request ids.
    next_id: u64,
    /// Collection of inflight requests.
    inflight_requests: HashMap<u64, ZkRessPeerRequest<P::Witness>>,
    /// Pending witness responses.
    pending_witnesses: FuturesUnordered<WitnessFut<P::Witness>>,
}

impl<P> ZkRessProtocolConnection<P>
where
    P: ZkRessProtocolProvider,
{
    /// Create new connection.
    pub fn new(
        provider: P,
        node_type: NodeType,
        peers_handle: PeersHandle,
        peer_id: PeerId,
        conn: ProtocolConnection,
        commands: UnboundedReceiverStream<ZkRessPeerRequest<P::Witness>>,
        active_connections: Arc<AtomicU64>,
    ) -> Self {
        Self {
            provider,
            node_type,
            peers_handle,
            peer_id,
            conn,
            commands,
            active_connections,
            node_type_sent: false,
            terminated: false,
            next_id: 0,
            inflight_requests: HashMap::default(),
            pending_witnesses: FuturesUnordered::new(),
        }
    }

    /// Returns the next request id
    const fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Report bad message from current peer.
    fn report_bad_message(&self) {
        self.peers_handle.reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
    }

    fn on_command(
        &mut self,
        command: ZkRessPeerRequest<P::Witness>,
    ) -> ZkRessProtocolMessage<P::Witness> {
        let next_id = self.next_id();
        let message = match &command {
            ZkRessPeerRequest::GetHeaders { request, .. } => {
                ZkRessProtocolMessage::get_headers(next_id, *request)
            }
            ZkRessPeerRequest::GetBlockBodies { request, .. } => {
                ZkRessProtocolMessage::get_block_bodies(next_id, request.clone())
            }
            ZkRessPeerRequest::GetWitness { block_hash, .. } => {
                ZkRessProtocolMessage::get_witness(next_id, *block_hash)
            }
        };
        self.inflight_requests.insert(next_id, command);
        message
    }
}

impl<P> ZkRessProtocolConnection<P>
where
    P: ZkRessProtocolProvider + Clone + 'static,
    P::Witness: Default,
{
    fn on_headers_request(&self, request: GetHeaders) -> Vec<Header> {
        match self.provider.headers(request) {
            Ok(headers) => headers,
            Err(error) => {
                trace!(target: "ress::net::connection", peer_id = %self.peer_id, ?request, %error, "error retrieving headers");
                Default::default()
            }
        }
    }

    fn on_block_bodies_request(&self, request: Vec<B256>) -> Vec<BlockBody> {
        match self.provider.block_bodies(request.clone()) {
            Ok(bodies) => bodies,
            Err(error) => {
                trace!(target: "ress::net::connection", peer_id = %self.peer_id, ?request, %error, "error retrieving block bodies");
                Default::default()
            }
        }
    }

    fn on_witness_response(
        &self,
        request: RequestPair<B256>,
        witness_result: ProviderResult<P::Witness>,
    ) -> ZkRessProtocolMessage<P::Witness> {
        let peer_id = self.peer_id;
        let block_hash = request.message;
        let witness = match witness_result {
            Ok(witness) => {
                trace!(target: "ress::net::connection", %peer_id, %block_hash, "witness found");
                witness
            }
            Err(error) => {
                trace!(target: "ress::net::connection", %peer_id, %block_hash, %error, "error retrieving witness");
                Default::default()
            }
        };
        ZkRessProtocolMessage::witness(request.request_id, witness)
    }

    fn on_ress_message(&mut self, msg: ZkRessProtocolMessage<P::Witness>) -> OnRessMessageOutcome {
        match msg.message {
            ZkRessMessage::NodeType(node_type) => {
                if !self.node_type.is_valid_connection(&node_type) {
                    // Note types are not compatible, terminate the connection.
                    return OnRessMessageOutcome::Terminate;
                }
            }
            ZkRessMessage::GetHeaders(req) => {
                let request = req.message;
                trace!(target: "ress::net::connection", peer_id = %self.peer_id, ?request, "serving headers");
                let header = self.on_headers_request(request);
                let response = ZkRessProtocolMessage::<P::Witness>::headers(req.request_id, header);
                return OnRessMessageOutcome::Response(response.encoded());
            }
            ZkRessMessage::GetBlockBodies(req) => {
                let request = req.message;
                trace!(target: "ress::net::connection", peer_id = %self.peer_id, ?request, "serving block bodies");
                let bodies = self.on_block_bodies_request(request);
                let response =
                    ZkRessProtocolMessage::<P::Witness>::block_bodies(req.request_id, bodies);
                return OnRessMessageOutcome::Response(response.encoded());
            }
            ZkRessMessage::GetWitness(req) => {
                let block_hash = req.message;
                trace!(target: "ress::net::connection", peer_id = %self.peer_id, %block_hash, "serving witness");
                let provider = self.provider.clone();
                self.pending_witnesses.push(Box::pin(async move {
                    let result = provider.witness(block_hash).await;
                    (req, result)
                }));
            }
            ZkRessMessage::Headers(res) => {
                if let Some(ZkRessPeerRequest::GetHeaders { tx, .. }) =
                    self.inflight_requests.remove(&res.request_id)
                {
                    let _ = tx.send(res.message);
                } else {
                    self.report_bad_message();
                }
            }
            ZkRessMessage::BlockBodies(res) => {
                if let Some(ZkRessPeerRequest::GetBlockBodies { tx, .. }) =
                    self.inflight_requests.remove(&res.request_id)
                {
                    let _ = tx.send(res.message);
                } else {
                    self.report_bad_message();
                }
            }
            ZkRessMessage::Witness(res) => {
                if let Some(ZkRessPeerRequest::GetWitness { tx, .. }) =
                    self.inflight_requests.remove(&res.request_id)
                {
                    let _ = tx.send(res.message);
                } else {
                    self.report_bad_message();
                }
            }
        };
        OnRessMessageOutcome::None
    }
}

impl<P: ZkRessProtocolProvider> Drop for ZkRessProtocolConnection<P> {
    fn drop(&mut self) {
        let _ = self
            .active_connections
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some(c.saturating_sub(1)));
    }
}

impl<P> Stream for ZkRessProtocolConnection<P>
where
    P: ZkRessProtocolProvider + Clone + Unpin + 'static,
    P::Witness: Default + fmt::Debug,
{
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.terminated {
            return Poll::Ready(None)
        }

        if !this.node_type_sent {
            this.node_type_sent = true;
            return Poll::Ready(Some(
                ZkRessProtocolMessage::<P::Witness>::node_type(this.node_type).encoded(),
            ))
        }

        'conn: loop {
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                let message = this.on_command(cmd);
                let encoded = message.encoded();
                trace!(target: "ress::net::connection", peer_id = %this.peer_id, ?message, encoded = alloy_primitives::hex::encode(&encoded), "Sending peer command");
                return Poll::Ready(Some(encoded));
            }

            if let Poll::Ready(Some((request, witness_result))) =
                this.pending_witnesses.poll_next_unpin(cx)
            {
                let response = this.on_witness_response(request, witness_result);
                return Poll::Ready(Some(response.encoded()));
            }

            if let Poll::Ready(maybe_msg) = this.conn.poll_next_unpin(cx) {
                let Some(next) = maybe_msg else { break 'conn };
                let msg = match ZkRessProtocolMessage::decode_message(&mut &next[..]) {
                    Ok(msg) => {
                        trace!(target: "ress::net::connection", peer_id = %this.peer_id, message = ?msg.message_type, "Processing message");
                        msg
                    }
                    Err(error) => {
                        trace!(target: "ress::net::connection", peer_id = %this.peer_id, %error, "Error decoding peer message");
                        this.report_bad_message();
                        continue;
                    }
                };

                match this.on_ress_message(msg) {
                    OnRessMessageOutcome::Response(bytes) => return Poll::Ready(Some(bytes)),
                    OnRessMessageOutcome::Terminate => break 'conn,
                    OnRessMessageOutcome::None => {}
                };

                continue;
            }

            return Poll::Pending;
        }

        // Terminating the connection.
        this.terminated = true;
        Poll::Ready(None)
    }
}

type WitnessFut<T> = Pin<Box<dyn Future<Output = (RequestPair<B256>, ProviderResult<T>)> + Send>>;

/// Ress peer request.
#[derive(Debug)]
pub enum ZkRessPeerRequest<T> {
    /// Get block headers.
    GetHeaders {
        /// The request for block headers.
        request: GetHeaders,
        /// The sender for the response.
        tx: oneshot::Sender<Vec<Header>>,
    },
    /// Get block bodies.
    GetBlockBodies {
        /// The request for block bodies.
        request: Vec<BlockHash>,
        /// The sender for the response.
        tx: oneshot::Sender<Vec<BlockBody>>,
    },
    /// Get witness for specific block.
    GetWitness {
        /// Target block hash that we want to get witness for.
        block_hash: BlockHash,
        /// The sender for the response.
        tx: oneshot::Sender<T>,
    },
}

enum OnRessMessageOutcome {
    /// Response to send to the peer.
    Response(BytesMut),
    /// Terminate the connection.
    Terminate,
    /// No action.
    None,
}
