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
    commands: UnboundedReceiverStream<ZkRessPeerRequest<P::Proof>>,
    /// The total number of active connections.
    active_connections: Arc<AtomicU64>,
    /// Flag indicating whether the node type was sent to the peer.
    node_type_sent: bool,
    /// Flag indicating whether this stream has previously been terminated.
    terminated: bool,
    /// Incremental counter for request ids.
    next_id: u64,
    /// Collection of inflight requests.
    inflight_requests: HashMap<u64, ZkRessPeerRequest<P::Proof>>,
    /// Pending proof responses.
    pending_proofs: FuturesUnordered<ProofFut<P::Proof>>,
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
        commands: UnboundedReceiverStream<ZkRessPeerRequest<P::Proof>>,
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
            pending_proofs: FuturesUnordered::new(),
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
        command: ZkRessPeerRequest<P::Proof>,
    ) -> ZkRessProtocolMessage<P::Proof> {
        let next_id = self.next_id();
        let message = match &command {
            ZkRessPeerRequest::GetHeaders { request, .. } => {
                ZkRessProtocolMessage::get_headers(next_id, *request)
            }
            ZkRessPeerRequest::GetBlockBodies { request, .. } => {
                ZkRessProtocolMessage::get_block_bodies(next_id, request.clone())
            }
            ZkRessPeerRequest::GetProof { block_hash, .. } => {
                ZkRessProtocolMessage::get_proof(next_id, *block_hash)
            }
        };
        self.inflight_requests.insert(next_id, command);
        message
    }
}

impl<P> ZkRessProtocolConnection<P>
where
    P: ZkRessProtocolProvider + Clone + 'static,
    P::Proof: Default,
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

    fn on_proof_response(
        &self,
        request: RequestPair<B256>,
        proof_result: ProviderResult<P::Proof>,
    ) -> ZkRessProtocolMessage<P::Proof> {
        let peer_id = self.peer_id;
        let block_hash = request.message;
        let proof = match proof_result {
            Ok(proof) => {
                trace!(target: "ress::net::connection", %peer_id, %block_hash, "proof found");
                proof
            }
            Err(error) => {
                trace!(target: "ress::net::connection", %peer_id, %block_hash, %error, "error retrieving proof");
                Default::default()
            }
        };
        ZkRessProtocolMessage::proof(request.request_id, proof)
    }

    fn on_ress_message(&mut self, msg: ZkRessProtocolMessage<P::Proof>) -> OnRessMessageOutcome {
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
                let response = ZkRessProtocolMessage::<P::Proof>::headers(req.request_id, header);
                return OnRessMessageOutcome::Response(response.encoded());
            }
            ZkRessMessage::GetBlockBodies(req) => {
                let request = req.message;
                trace!(target: "ress::net::connection", peer_id = %self.peer_id, ?request, "serving block bodies");
                let bodies = self.on_block_bodies_request(request);
                let response =
                    ZkRessProtocolMessage::<P::Proof>::block_bodies(req.request_id, bodies);
                return OnRessMessageOutcome::Response(response.encoded());
            }
            ZkRessMessage::GetProof(req) => {
                let block_hash = req.message;
                trace!(target: "ress::net::connection", peer_id = %self.peer_id, %block_hash, "serving proof");
                let provider = self.provider.clone();
                self.pending_proofs.push(Box::pin(async move {
                    let result = provider.proof(block_hash).await;
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
            ZkRessMessage::Proof(res) => {
                if let Some(ZkRessPeerRequest::GetProof { tx, .. }) =
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
    P::Proof: Default + fmt::Debug,
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
                ZkRessProtocolMessage::<P::Proof>::node_type(this.node_type).encoded(),
            ))
        }

        'conn: loop {
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                let message = this.on_command(cmd);
                let encoded = message.encoded();
                trace!(target: "ress::net::connection", peer_id = %this.peer_id, ?message, encoded = alloy_primitives::hex::encode(&encoded), "Sending peer command");
                return Poll::Ready(Some(encoded));
            }

            if let Poll::Ready(Some((request, proof_result))) =
                this.pending_proofs.poll_next_unpin(cx)
            {
                let response = this.on_proof_response(request, proof_result);
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

type ProofFut<T> = Pin<Box<dyn Future<Output = (RequestPair<B256>, ProviderResult<T>)> + Send>>;

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
    /// Get proof for specific block.
    GetProof {
        /// Target block hash that we want to get proof for.
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
