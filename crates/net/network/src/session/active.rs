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
    capability::{Capabilities}, EthMessage, EthStream, P2PStream,
};
use reth_primitives::PeerId;
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Instant,
};
use tokio::{
    net::TcpStream,
    sync::{mpsc},
};
use tokio_stream::wrappers::ReceiverStream;

/// The type that advances an established session by listening for incoming messages (from local
/// node or read from connection) and emitting events back to the [`SessionHandler`].
pub(crate) struct ActiveSession {
    /// Keeps track of request ids.
    pub(crate) next_id: usize,
    /// The underlying connection.
    pub(crate) conn: EthStream<P2PStream<ECIESStream<TcpStream>>>,
    /// Identifier of the node we're connected to.
    pub(crate) remote_peer_id: PeerId,
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
    /// Pending disconnect message
    pub(crate) is_gracefully_disconnecting: bool,
}

impl ActiveSession {
    /// Handle an incoming peer request.
    fn on_peer_request(&mut self, _req: PeerRequest) {}

    /// Handle a message read from the connection.
    fn on_incoming(&mut self, _msg: EthMessage) {}

    /// Handle a message received from the internal network
    fn on_peer_message(&mut self, _msg: PeerMessage) {}

    /// Handle a Response
    fn handle_outgoing_response(&mut self, _id: u64, _resp: PeerResponseResult) {}
}

impl Future for ActiveSession {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.get_mut();

        if this.is_gracefully_disconnecting {
            // try to close the flush out the remaining Disconnect message
            let _ = ready!(this.conn.poll_close_unpin(cx));
            // this.to_session.clone().send(
            // );
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
                            SessionCommand::Disconnect { reason: _ } => {
                                // TODO queue in disconnect message
                                this.is_gracefully_disconnecting = true;
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
                        this.received_requests.push(req);
                    }
                    Poll::Ready(Ok(resp)) => {
                        this.handle_outgoing_response(req.id, resp);
                    }
                    Poll::Ready(Err(_)) => {}
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
                        // disconnected
                        this.is_gracefully_disconnecting = true;
                        return Poll::Pending
                    }
                    Poll::Ready(Some(res)) => {
                        progress = true;
                        match res {
                            Ok(msg) => {
                                // decode and handle message
                                this.on_incoming(msg);
                            }
                            Err(_err) => return Poll::Ready(()),
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
