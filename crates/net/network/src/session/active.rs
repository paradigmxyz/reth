//! Represents an established session.

use crate::{
    message::{PeerMessage, PeerRequest, PeerResponse, PeerResponseResult},
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
    error::{EthStreamError, P2PStreamError},
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
use tokio::{net::TcpStream, sync::mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;
use reth_interfaces::p2p::error::RequestResult;

/// The type that advances an established session by listening for incoming messages (from local
/// node or read from connection) and emitting events back to the [`SessionsManager`].
///
/// It listens for
///    - incoming commands from the [`SessionsManager`]
///    - incoming requests via the request channel
///    - responses for handled ETH requests received from the remote peer.
pub(crate) struct ActiveSession {
    /// Keeps track of request ids.
    pub(crate) next_id: usize,
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
    pub(crate) buffered_outgoing: VecDeque<EthMessage>,
}

impl ActiveSession {
    /// Returns `true` if the session is currently in the process of disconnecting
    fn is_disconnecting(&self) -> bool {
        self.conn.inner().is_disconnecting()
    }

    /// Handle an incoming peer request.
    fn on_peer_request(&mut self, _req: PeerRequest) {}

    /// Handle a message read from the connection.
    fn on_incoming(&mut self, _msg: EthMessage) {}

    /// Handle a message received from the internal network
    fn on_peer_message(&mut self, msg: PeerMessage) {
        match msg {
            PeerMessage::NewBlockHashes(_) => {}
            PeerMessage::NewBlock(block) => {
                self.buffered_outgoing.push_back(
                    EthMessage::NewBlock(Box::new(block))
                )
            }
            PeerMessage::Transactions(tx) => {}
            PeerMessage::PooledTransactions(_) => {}
            PeerMessage::EthRequest(_) => {}
            PeerMessage::Other(_) => {}
        }
    }

    /// Handle a Response to the peer
    fn handle_outgoing_response(&mut self, id: u64, resp: PeerResponseResult) {
        match resp.try_into_message(id) {
            Ok(msg) => {
                self.buffered_outgoing.push_back(msg);
            }
            Err(err) => {
                error!(?err, target = "net", "Failed to respond to received request");
            }
        }
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
        let mut this = self.get_mut();

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
                        this.handle_outgoing_response(req.id, resp);
                    }
                    Poll::Ready(Err(_)) => {
                        // ignore on error
                    }
                }
            }

            // Flush messages
            while this.conn.poll_ready_unpin(cx).is_ready() {
                if let Some(msg) = this.buffered_outgoing.pop_front() {
                    progress = true;
                    if let Err(_err) = this.conn.start_send_unpin(msg) {
                        return Poll::Ready(())
                    }
                } else {
                    break
                }
            }

            loop {
                match this.conn.poll_next_unpin(cx) {
                    Poll::Pending => break,
                    Poll::Ready(None) => {
                        return Poll::Pending
                    }
                    Poll::Ready(Some(res)) => {
                        progress = true;
                        match res {
                            Ok(msg) => {
                                // decode and handle message
                                this.on_incoming(msg);
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
    id: u64,
    /// Receiver half of the channel that's supposed to receive the proper response.
    rx: PeerResponse,
    /// Timestamp when we read this msg from the wire.
    received: Instant,
}
