//! Represents an established session.

use crate::{
    message::{NewBlockMessage, PeerMessage, PeerRequest, PeerResponse, PeerResponseResult},
    session::{
        handle::{ActiveSessionMessage, SessionCommand},
        SessionId,
    },
};
use fnv::FnvHashMap;
use futures::{stream::Fuse, SinkExt, StreamExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    capability::Capabilities,
    error::{EthStreamError, HandshakeError},
    message::{EthBroadcastMessage, RequestPair},
    DisconnectReason, EthMessage, EthStream, P2PStream,
};

use reth_primitives::PeerId;
use std::{
    collections::VecDeque,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Instant,
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, warn};

/// The type that advances an established session by listening for incoming messages (from local
/// node or read from connection) and emitting events back to the [`SessionsManager`].
///
/// It listens for
///    - incoming commands from the [`SessionsManager`]
///    - incoming requests via the request channel
///    - responses for handled ETH requests received from the remote peer.
pub(crate) struct ActiveSession {
    /// Keeps track of request ids.
    pub(crate) next_id: u64,
    /// The underlying connection.
    pub(crate) conn: EthStream<P2PStream<ECIESStream<TcpStream>>>,
    /// Identifier of the node we're connected to.
    pub(crate) remote_peer_id: PeerId,
    /// The address we're connected to.
    pub(crate) remote_addr: SocketAddr,
    /// All capabilities the peer announced
    pub(crate) remote_capabilities: Arc<Capabilities>,
    /// Internal identifier of this session
    pub(crate) session_id: SessionId,
    /// Incoming commands from the manager
    pub(crate) commands_rx: ReceiverStream<SessionCommand>,
    /// Sink to send messages to the [`SessionManager`].
    pub(crate) to_session: mpsc::Sender<ActiveSessionMessage>,
    /// Incoming request to send to delegate to the remote peer.
    pub(crate) request_tx: Fuse<ReceiverStream<PeerRequest>>,
    /// All requests sent to the remote peer we're waiting on a response
    pub(crate) inflight_requests: FnvHashMap<u64, PeerRequest>,
    /// All requests that were sent by the remote peer.
    pub(crate) received_requests: Vec<ReceivedRequest>,
    /// Buffered messages that should be handled and sent to the peer.
    pub(crate) queued_outgoing: VecDeque<OutgoingMessage>,
}

impl ActiveSession {
    /// Returns `true` if the session is currently in the process of disconnecting
    fn is_disconnecting(&self) -> bool {
        self.conn.inner().is_disconnecting()
    }

    /// Returns the next request id
    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Handle a message read from the connection.
    ///
    /// Returns an error if the message is considered to be in violation of the protocol
    fn on_incoming(&mut self, msg: EthMessage) -> Option<EthStreamError> {
        /// A macro that handles an incoming request
        /// This creates a new channel and tries to send the sender half to the session while
        /// storing to receiver half internally so the pending response can be polled.
        macro_rules! on_request {
            ($req:ident, $resp_item:ident, $req_item:ident) => {
                let RequestPair { request_id, message: request } = $req;
                let (tx, response) = oneshot::channel();
                let received = ReceivedRequest {
                    request_id,
                    rx: PeerResponse::$resp_item { response },
                    received: Instant::now(),
                };
                if self
                    .try_emit_message(PeerMessage::EthRequest(PeerRequest::$req_item {
                        request,
                        response: tx,
                    }))
                    .is_ok()
                {
                    self.received_requests.push(received);
                }
            };
        }

        /// Processes a response received from the peer
        macro_rules! on_response {
            ($resp:ident, $item:ident) => {
                let RequestPair { request_id, message } = $resp;
                #[allow(clippy::collapsible_match)]
                if let Some(resp) = self.inflight_requests.remove(&request_id) {
                    if let PeerRequest::$item { response, .. } = resp {
                        let _ = response.send(Ok(message));
                    } else {
                        // TODO handle bad response
                    }
                } else {
                    // TODO handle unexpected response
                }
            };
        }

        match msg {
            EthMessage::Status(_) => {
                return Some(EthStreamError::HandshakeError(HandshakeError::StatusNotInHandshake))
            }
            EthMessage::NewBlockHashes(msg) => {
                self.emit_message(PeerMessage::NewBlockHashes(Arc::new(msg)));
            }
            EthMessage::NewBlock(msg) => {
                let block =
                    NewBlockMessage { hash: msg.block.header.hash_slow(), block: Arc::new(*msg) };
                self.emit_message(PeerMessage::NewBlock(block));
            }
            EthMessage::Transactions(msg) => {
                self.emit_message(PeerMessage::Transactions(Arc::new(msg)));
            }
            EthMessage::NewPooledTransactionHashes(msg) => {
                self.emit_message(PeerMessage::PooledTransactions(Arc::new(msg)));
            }
            EthMessage::GetBlockHeaders(req) => {
                on_request!(req, BlockHeaders, GetBlockHeaders);
            }
            EthMessage::BlockHeaders(resp) => {
                on_response!(resp, GetBlockHeaders);
            }
            EthMessage::GetBlockBodies(req) => {
                on_request!(req, BlockBodies, GetBlockBodies);
            }
            EthMessage::BlockBodies(resp) => {
                on_response!(resp, GetBlockBodies);
            }
            EthMessage::GetPooledTransactions(req) => {
                on_request!(req, PooledTransactions, GetPooledTransactions);
            }
            EthMessage::PooledTransactions(resp) => {
                on_response!(resp, GetPooledTransactions);
            }
            EthMessage::GetNodeData(req) => {
                on_request!(req, NodeData, GetNodeData);
            }
            EthMessage::NodeData(resp) => {
                on_response!(resp, GetNodeData);
            }
            EthMessage::GetReceipts(req) => {
                on_request!(req, Receipts, GetReceipts);
            }
            EthMessage::Receipts(resp) => {
                on_response!(resp, GetReceipts);
            }
        };

        None
    }

    /// Handle an incoming peer request.
    fn on_peer_request(&mut self, req: PeerRequest) {
        let request_id = self.next_id();
        let msg = req.create_request_message(request_id);
        self.queued_outgoing.push_back(msg.into());
        self.inflight_requests.insert(request_id, req);
    }

    /// Handle a message received from the internal network
    fn on_peer_message(&mut self, msg: PeerMessage) {
        match msg {
            PeerMessage::NewBlockHashes(msg) => {
                self.queued_outgoing.push_back(EthBroadcastMessage::NewBlockHashes(msg).into());
            }
            PeerMessage::NewBlock(msg) => {
                self.queued_outgoing.push_back(EthBroadcastMessage::NewBlock(msg.block).into());
            }
            PeerMessage::Transactions(msg) => {
                self.queued_outgoing.push_back(EthBroadcastMessage::Transactions(msg).into());
            }
            PeerMessage::PooledTransactions(msg) => {
                self.queued_outgoing
                    .push_back(EthBroadcastMessage::NewPooledTransactionHashes(msg).into());
            }
            PeerMessage::EthRequest(req) => {
                self.on_peer_request(req);
            }
            PeerMessage::Other(_) => {}
        }
    }

    /// Handle a Response to the peer
    fn handle_outgoing_response(&mut self, id: u64, resp: PeerResponseResult) {
        match resp.try_into_message(id) {
            Ok(msg) => {
                self.queued_outgoing.push_back(msg.into());
            }
            Err(err) => {
                error!(?err, target = "net", "Failed to respond to received request");
            }
        }
    }

    /// Send a message back to the [`SessionsManager`]
    fn emit_message(&self, message: PeerMessage) {
        let _ = self.try_emit_message(message).map_err(|err| {
            warn!(
            %err,
                    target = "net",
                    "dropping incoming message",
                );
        });
    }

    /// Send a message back to the [`SessionsManager`]
    fn try_emit_message(
        &self,
        message: PeerMessage,
    ) -> Result<(), mpsc::error::TrySendError<ActiveSessionMessage>> {
        self.to_session
            .try_send(ActiveSessionMessage::ValidMessage { peer_id: self.remote_peer_id, message })
    }

    /// Report back that this session has been closed.
    fn disconnect(&self) {
        // NOTE: we clone here so there's enough capacity to deliver this message
        let _ = self.to_session.clone().try_send(ActiveSessionMessage::Disconnected {
            peer_id: self.remote_peer_id,
            remote_addr: self.remote_addr,
        });
    }

    /// Report back that this session has been closed due to an error
    fn close_on_error(&self, error: EthStreamError) {
        // NOTE: we clone here so there's enough capacity to deliver this message
        let _ = self.to_session.clone().try_send(ActiveSessionMessage::ClosedOnConnectionError {
            peer_id: self.remote_peer_id,
            remote_addr: self.remote_addr,
            error,
        });
    }
}

impl Future for ActiveSession {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if this.is_disconnecting() {
            // try to close the flush out the remaining Disconnect message
            let _ = ready!(this.conn.poll_close_unpin(cx));
            this.disconnect();
            return Poll::Ready(())
        }

        loop {
            let mut progress = false;

            // we prioritize incoming commands sent from the session manager
            loop {
                match this.commands_rx.poll_next_unpin(cx) {
                    Poll::Pending => break,
                    Poll::Ready(None) => {
                        // this is only possible when the manager was dropped, in which case we also
                        // terminate this session
                        return Poll::Ready(())
                    }
                    Poll::Ready(Some(cmd)) => {
                        progress = true;
                        match cmd {
                            SessionCommand::Disconnect { reason } => {
                                let reason =
                                    reason.unwrap_or(DisconnectReason::DisconnectRequested);
                                this.conn.inner_mut().start_disconnect(reason);
                            }
                            SessionCommand::Message(msg) => {
                                this.on_peer_message(msg);
                            }
                        }
                    }
                }
            }

            while let Poll::Ready(Some(req)) = this.request_tx.poll_next_unpin(cx) {
                progress = true;
                this.on_peer_request(req);
            }

            // Advance all active requests.
            // We remove each request one by one and add them back.
            for idx in (0..this.received_requests.len()).rev() {
                let mut req = this.received_requests.swap_remove(idx);
                match req.rx.poll(cx) {
                    Poll::Pending => {
                        // not ready yet
                        this.received_requests.push(req);
                    }
                    Poll::Ready(Ok(resp)) => {
                        this.handle_outgoing_response(req.request_id, resp);
                    }
                    Poll::Ready(Err(_)) => {
                        // ignore on error
                    }
                }
            }

            // Send messages by advancing the sink and queuing in buffered messages
            while this.conn.poll_ready_unpin(cx).is_ready() {
                if let Some(msg) = this.queued_outgoing.pop_front() {
                    progress = true;
                    let res = match msg {
                        OutgoingMessage::Eth(msg) => this.conn.start_send_unpin(msg),
                        OutgoingMessage::Broadcast(msg) => this.conn.start_send_broadcast(msg),
                    };
                    if let Err(_err) = res {
                        return Poll::Ready(())
                    }
                } else {
                    break
                }
            }

            loop {
                match this.conn.poll_next_unpin(cx) {
                    Poll::Pending => break,
                    Poll::Ready(None) => return Poll::Pending,
                    Poll::Ready(Some(res)) => {
                        progress = true;
                        match res {
                            Ok(msg) => {
                                // decode and handle message
                                if let Some(err) = this.on_incoming(msg) {
                                    this.close_on_error(err);
                                    return Poll::Ready(())
                                }
                            }
                            Err(err) => {
                                this.close_on_error(err);
                                return Poll::Ready(())
                            }
                        }
                    }
                }
            }

            if !progress {
                return Poll::Pending
            }
        }
    }
}

/// Tracks a request received from the peer
pub(crate) struct ReceivedRequest {
    /// Protocol Identifier
    request_id: u64,
    /// Receiver half of the channel that's supposed to receive the proper response.
    rx: PeerResponse,
    /// Timestamp when we read this msg from the wire.
    received: Instant,
}

/// Outgoing messages that can be sent over the wire.
pub(crate) enum OutgoingMessage {
    /// A message that is owned.
    Eth(EthMessage),
    /// A message that may be shared by multiple sessions.
    Broadcast(EthBroadcastMessage),
}

impl From<EthMessage> for OutgoingMessage {
    fn from(value: EthMessage) -> Self {
        OutgoingMessage::Eth(value)
    }
}

impl From<EthBroadcastMessage> for OutgoingMessage {
    fn from(value: EthBroadcastMessage) -> Self {
        OutgoingMessage::Broadcast(value)
    }
}
