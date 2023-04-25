//! Represents an established session.

use crate::{
    message::{NewBlockMessage, PeerMessage, PeerRequest, PeerResponse, PeerResponseResult},
    session::{
        config::INITIAL_REQUEST_TIMEOUT,
        handle::{ActiveSessionMessage, SessionCommand},
        SessionId,
    },
};
use core::sync::atomic::Ordering;
use fnv::FnvHashMap;
use futures::{stream::Fuse, SinkExt, StreamExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    capability::Capabilities,
    errors::{EthHandshakeError, EthStreamError, P2PStreamError},
    message::{EthBroadcastMessage, RequestPair},
    DisconnectReason, EthMessage, EthStream, P2PStream,
};
use reth_interfaces::p2p::error::RequestError;
use reth_metrics_common::metered_sender::MeteredSender;
use reth_net_common::bandwidth_meter::MeteredStream;
use reth_primitives::PeerId;
use std::{
    collections::VecDeque,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{atomic::AtomicU64, Arc},
    task::{ready, Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpStream,
    sync::{mpsc::error::TrySendError, oneshot},
    time::Interval,
};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, trace, warn};

/// Constants for timeout updating

/// Minimum timeout value
const MINIMUM_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum timeout value
const MAXIMUM_TIMEOUT: Duration = INITIAL_REQUEST_TIMEOUT;
/// How much the new measurements affect the current timeout (X percent)
const SAMPLE_IMPACT: f64 = 0.1;
/// Amount of RTTs before timeout
const TIMEOUT_SCALING: u32 = 3;

/// The type that advances an established session by listening for incoming messages (from local
/// node or read from connection) and emitting events back to the
/// [`SessionManager`](super::SessionManager).
///
/// It listens for
///    - incoming commands from the [`SessionManager`](super::SessionManager)
///    - incoming _internal_ requests/broadcasts via the request/command channel
///    - incoming requests/broadcasts _from remote_ via the connection
///    - responses for handled ETH requests received from the remote peer.
#[allow(unused)]
pub(crate) struct ActiveSession {
    /// Keeps track of request ids.
    pub(crate) next_id: u64,
    /// The underlying connection.
    pub(crate) conn: EthStream<P2PStream<ECIESStream<MeteredStream<TcpStream>>>>,
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
    /// Sink to send messages to the [`SessionManager`](super::SessionManager).
    pub(crate) to_session: MeteredSender<ActiveSessionMessage>,
    /// A message that needs to be delivered to the session manager
    pub(crate) pending_message_to_session: Option<ActiveSessionMessage>,
    /// Incoming request to send to delegate to the remote peer.
    pub(crate) internal_request_tx: Fuse<ReceiverStream<PeerRequest>>,
    /// All requests sent to the remote peer we're waiting on a response
    pub(crate) inflight_requests: FnvHashMap<u64, InflightRequest>,
    /// All requests that were sent by the remote peer.
    pub(crate) received_requests_from_remote: Vec<ReceivedRequest>,
    /// Buffered messages that should be handled and sent to the peer.
    pub(crate) queued_outgoing: VecDeque<OutgoingMessage>,
    /// The maximum time we wait for a response from a peer.
    pub(crate) internal_request_timeout: Arc<AtomicU64>,
    /// Interval when to check for timed out requests.
    pub(crate) internal_request_timeout_interval: Interval,
    /// If an [ActiveSession] does not receive a response at all within this duration then it is
    /// considered a protocol violation and the session will initiate a drop.
    pub(crate) protocol_breach_request_timeout: Duration,
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
    /// Returns an error if the message is considered to be in violation of the protocol.
    fn on_incoming(&mut self, msg: EthMessage) -> OnIncomingMessageOutcome {
        /// A macro that handles an incoming request
        /// This creates a new channel and tries to send the sender half to the session while
        /// storing the receiver half internally so the pending response can be polled.
        macro_rules! on_request {
            ($req:ident, $resp_item:ident, $req_item:ident) => {{
                let RequestPair { request_id, message: request } = $req;
                let (tx, response) = oneshot::channel();
                let received = ReceivedRequest {
                    request_id,
                    rx: PeerResponse::$resp_item { response },
                    received: Instant::now(),
                };
                self.received_requests_from_remote.push(received);
                self.try_emit_request(PeerMessage::EthRequest(PeerRequest::$req_item {
                    request,
                    response: tx,
                }))
                .into()
            }};
        }

        /// Processes a response received from the peer
        macro_rules! on_response {
            ($resp:ident, $item:ident) => {{
                let RequestPair { request_id, message } = $resp;
                #[allow(clippy::collapsible_match)]
                if let Some(req) = self.inflight_requests.remove(&request_id) {
                    match req.request {
                        RequestState::Waiting(PeerRequest::$item { response, .. }) => {
                            let _ = response.send(Ok(message));
                            self.update_request_timeout(req.timestamp, Instant::now());
                        }
                        RequestState::Waiting(request) => {
                            request.send_bad_response();
                        }
                        RequestState::TimedOut => {
                            // request was already timed out internally
                            self.update_request_timeout(req.timestamp, Instant::now());
                        }
                    };
                } else {
                    // we received a response to a request we never sent
                    self.on_bad_message();
                }

                OnIncomingMessageOutcome::Ok
            }};
        }

        match msg {
            message @ EthMessage::Status(_) => OnIncomingMessageOutcome::BadMessage {
                error: EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake),
                message,
            },
            EthMessage::NewBlockHashes(msg) => {
                self.try_emit_broadcast(PeerMessage::NewBlockHashes(msg)).into()
            }
            EthMessage::NewBlock(msg) => {
                let block =
                    NewBlockMessage { hash: msg.block.header.hash_slow(), block: Arc::new(*msg) };
                self.try_emit_broadcast(PeerMessage::NewBlock(block)).into()
            }
            EthMessage::Transactions(msg) => {
                self.try_emit_broadcast(PeerMessage::ReceivedTransaction(msg)).into()
            }
            EthMessage::NewPooledTransactionHashes66(msg) => {
                self.try_emit_broadcast(PeerMessage::PooledTransactions(msg.into())).into()
            }
            EthMessage::NewPooledTransactionHashes68(msg) => {
                if msg.hashes.len() != msg.types.len() || msg.hashes.len() != msg.sizes.len() {
                    return OnIncomingMessageOutcome::BadMessage {
                        error: EthStreamError::TransactionHashesInvalidLenOfFields {
                            hashes_len: msg.hashes.len(),
                            types_len: msg.types.len(),
                            sizes_len: msg.sizes.len(),
                        },
                        message: EthMessage::NewPooledTransactionHashes68(msg),
                    }
                }
                self.try_emit_broadcast(PeerMessage::PooledTransactions(msg.into())).into()
            }
            EthMessage::GetBlockHeaders(req) => {
                on_request!(req, BlockHeaders, GetBlockHeaders)
            }
            EthMessage::BlockHeaders(resp) => {
                on_response!(resp, GetBlockHeaders)
            }
            EthMessage::GetBlockBodies(req) => {
                on_request!(req, BlockBodies, GetBlockBodies)
            }
            EthMessage::BlockBodies(resp) => {
                on_response!(resp, GetBlockBodies)
            }
            EthMessage::GetPooledTransactions(req) => {
                on_request!(req, PooledTransactions, GetPooledTransactions)
            }
            EthMessage::PooledTransactions(resp) => {
                on_response!(resp, GetPooledTransactions)
            }
            EthMessage::GetNodeData(req) => {
                on_request!(req, NodeData, GetNodeData)
            }
            EthMessage::NodeData(resp) => {
                on_response!(resp, GetNodeData)
            }
            EthMessage::GetReceipts(req) => {
                on_request!(req, Receipts, GetReceipts)
            }
            EthMessage::Receipts(resp) => {
                on_response!(resp, GetReceipts)
            }
        }
    }

    /// Handle an internal peer request that will be sent to the remote.
    fn on_internal_peer_request(&mut self, request: PeerRequest, deadline: Instant) {
        let request_id = self.next_id();
        let msg = request.create_request_message(request_id);
        self.queued_outgoing.push_back(msg.into());
        let req = InflightRequest {
            request: RequestState::Waiting(request),
            timestamp: Instant::now(),
            deadline,
        };
        self.inflight_requests.insert(request_id, req);
    }

    /// Handle a message received from the internal network
    fn on_peer_message(&mut self, msg: PeerMessage) {
        match msg {
            PeerMessage::NewBlockHashes(msg) => {
                self.queued_outgoing.push_back(EthMessage::NewBlockHashes(msg).into());
            }
            PeerMessage::NewBlock(msg) => {
                self.queued_outgoing.push_back(EthBroadcastMessage::NewBlock(msg.block).into());
            }
            PeerMessage::PooledTransactions(msg) => {
                if msg.is_valid_for_version(self.conn.version()) {
                    self.queued_outgoing.push_back(EthMessage::from(msg).into());
                }
            }
            PeerMessage::EthRequest(req) => {
                let deadline = self.request_deadline();
                self.on_internal_peer_request(req, deadline);
            }
            PeerMessage::SendTransactions(msg) => {
                self.queued_outgoing.push_back(EthBroadcastMessage::Transactions(msg).into());
            }
            PeerMessage::ReceivedTransaction(_) => {
                unreachable!("Not emitted by network")
            }
            PeerMessage::Other(other) => {
                error!(target : "net::session", message_id=%other.id, "Ignoring unsupported message");
            }
        }
    }

    /// Returns the deadline timestamp at which the request times out
    fn request_deadline(&self) -> Instant {
        Instant::now() +
            Duration::from_millis(self.internal_request_timeout.load(Ordering::Relaxed))
    }

    /// Handle a Response to the peer
    fn handle_outgoing_response(&mut self, id: u64, resp: PeerResponseResult) {
        match resp.try_into_message(id) {
            Ok(msg) => {
                self.queued_outgoing.push_back(msg.into());
            }
            Err(err) => {
                error!(target : "net", ?err, "Failed to respond to received request");
            }
        }
    }

    /// Send a message back to the [`SessionManager`](super::SessionManager).
    ///
    /// Returns the message if the bounded channel is currently unable to handle this message.
    #[allow(clippy::result_large_err)]
    fn try_emit_broadcast(&self, message: PeerMessage) -> Result<(), ActiveSessionMessage> {
        match self
            .to_session
            .try_send(ActiveSessionMessage::ValidMessage { peer_id: self.remote_peer_id, message })
        {
            Ok(_) => Ok(()),
            Err(err) => {
                trace!(
                    target : "net",
                    %err,
                    "no capacity for incoming broadcast",
                );
                match err {
                    TrySendError::Full(msg) => Err(msg),
                    TrySendError::Closed(_) => Ok(()),
                }
            }
        }
    }

    /// Send a message back to the [`SessionManager`](super::SessionManager)
    /// covering both broadcasts and incoming requests.
    ///
    /// Returns the message if the bounded channel is currently unable to handle this message.
    #[allow(clippy::result_large_err)]
    fn try_emit_request(&self, message: PeerMessage) -> Result<(), ActiveSessionMessage> {
        match self
            .to_session
            .try_send(ActiveSessionMessage::ValidMessage { peer_id: self.remote_peer_id, message })
        {
            Ok(_) => Ok(()),
            Err(err) => {
                trace!(
                    target : "net",
                    %err,
                    "no capacity for incoming request",
                );
                match err {
                    TrySendError::Full(msg) => Err(msg),
                    TrySendError::Closed(_) => {
                        // Note: this would mean the `SessionManager` was dropped, which is already
                        // handled by checking if the command receiver channel has been closed.
                        Ok(())
                    }
                }
            }
        }
    }

    /// Notify the manager that the peer sent a bad message
    fn on_bad_message(&self) {
        let _ = self
            .to_session
            .try_send(ActiveSessionMessage::BadMessage { peer_id: self.remote_peer_id });
    }

    /// Report back that this session has been closed.
    fn emit_disconnect(&self) {
        trace!(target: "net::session", remote_peer_id=?self.remote_peer_id, "emitting disconnect");
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

    /// Starts the disconnect process
    fn start_disconnect(&mut self, reason: DisconnectReason) -> Result<(), EthStreamError> {
        self.conn
            .inner_mut()
            .start_disconnect(reason)
            .map_err(P2PStreamError::from)
            .map_err(Into::into)
    }

    /// Flushes the disconnect message and emits the corresponding message
    fn poll_disconnect(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        debug_assert!(self.is_disconnecting(), "not disconnecting");

        // try to close the flush out the remaining Disconnect message
        let _ = ready!(self.conn.poll_close_unpin(cx));
        self.emit_disconnect();
        Poll::Ready(())
    }

    /// Attempts to disconnect by sending the given disconnect reason
    fn try_disconnect(&mut self, reason: DisconnectReason, cx: &mut Context<'_>) -> Poll<()> {
        match self.start_disconnect(reason) {
            Ok(()) => {
                // we're done
                self.poll_disconnect(cx)
            }
            Err(err) => {
                error!(target: "net::session", ?err, remote_peer_id=?self.remote_peer_id, "could not send disconnect");
                self.close_on_error(err);
                Poll::Ready(())
            }
        }
    }

    /// Checks for _internally_ timed out requests.
    ///
    /// If a requests misses its deadline, then it is timed out internally.
    /// If a request misses the `protocol_breach_request_timeout` then this session is considered in
    /// protocol violation and will close.
    ///
    /// Returns `true` if a peer missed the `protocol_breach_request_timeout`, in which case the
    /// session should be terminated.
    #[must_use]
    fn check_timed_out_requests(&mut self, now: Instant) -> bool {
        for (id, req) in self.inflight_requests.iter_mut() {
            if req.is_timed_out(now) {
                if req.is_waiting() {
                    warn!(target: "net::session", ?id, remote_peer_id=?self.remote_peer_id, "timed out outgoing request");
                    req.timeout();
                } else if now - req.timestamp > self.protocol_breach_request_timeout {
                    return true
                }
            }
        }

        false
    }

    /// Updates the request timeout with a request's timestamps
    fn update_request_timeout(&mut self, sent: Instant, received: Instant) {
        let elapsed = received.saturating_duration_since(sent);

        let current = Duration::from_millis(self.internal_request_timeout.load(Ordering::Relaxed));
        let request_timeout = calculate_new_timeout(current, elapsed);
        self.internal_request_timeout.store(request_timeout.as_millis() as u64, Ordering::Relaxed);
        self.internal_request_timeout_interval = tokio::time::interval(request_timeout);
    }
}

/// Calculates a new timeout using an updated estimation of the RTT
#[inline]
fn calculate_new_timeout(current_timeout: Duration, estimated_rtt: Duration) -> Duration {
    let new_timeout = estimated_rtt.mul_f64(SAMPLE_IMPACT) * TIMEOUT_SCALING;

    // this dampens sudden changes by taking a weighted mean of the old and new values
    let smoothened_timeout = current_timeout.mul_f64(1.0 - SAMPLE_IMPACT) + new_timeout;

    smoothened_timeout.clamp(MINIMUM_TIMEOUT, MAXIMUM_TIMEOUT)
}

impl Future for ActiveSession {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if this.is_disconnecting() {
            return this.poll_disconnect(cx)
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
                                info!(target: "net::session", ?reason, remote_peer_id=?this.remote_peer_id, "session received disconnect command");
                                let reason =
                                    reason.unwrap_or(DisconnectReason::DisconnectRequested);

                                return this.try_disconnect(reason, cx)
                            }
                            SessionCommand::Message(msg) => {
                                this.on_peer_message(msg);
                            }
                        }
                    }
                }
            }

            let deadline = this.request_deadline();

            while let Poll::Ready(Some(req)) = this.internal_request_tx.poll_next_unpin(cx) {
                progress = true;
                this.on_internal_peer_request(req, deadline);
            }

            // Advance all active requests.
            // We remove each request one by one and add them back.
            for idx in (0..this.received_requests_from_remote.len()).rev() {
                let mut req = this.received_requests_from_remote.swap_remove(idx);
                match req.rx.poll(cx) {
                    Poll::Pending => {
                        // not ready yet
                        this.received_requests_from_remote.push(req);
                    }
                    Poll::Ready(resp) => {
                        this.handle_outgoing_response(req.request_id, resp);
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
                    if let Err(err) = res {
                        error!(target: "net::session", ?err,  remote_peer_id=?this.remote_peer_id, "failed to send message");
                        // notify the manager
                        this.close_on_error(err);
                        return Poll::Ready(())
                    }
                } else {
                    // no more messages to send over the wire
                    break
                }
            }

            // read incoming messages from the wire
            'receive: loop {
                // try to resend the pending message that we could not send because the channel was
                // full.
                if let Some(msg) = this.pending_message_to_session.take() {
                    match this.to_session.try_send(msg) {
                        Ok(_) => {}
                        Err(err) => {
                            match err {
                                TrySendError::Full(msg) => {
                                    this.pending_message_to_session = Some(msg);
                                    // ensure we're woken up again
                                    cx.waker().wake_by_ref();
                                    break 'receive
                                }
                                TrySendError::Closed(_) => {}
                            }
                        }
                    }
                }

                match this.conn.poll_next_unpin(cx) {
                    Poll::Pending => break,
                    Poll::Ready(None) => {
                        if this.is_disconnecting() {
                            break
                        } else {
                            debug!(target: "net::session", remote_peer_id=?this.remote_peer_id, "eth stream completed");
                            this.emit_disconnect();
                            return Poll::Ready(())
                        }
                    }
                    Poll::Ready(Some(res)) => {
                        match res {
                            Ok(msg) => {
                                trace!(target: "net::session", msg_id=?msg.message_id(), remote_peer_id=?this.remote_peer_id, "received eth message");
                                // decode and handle message
                                match this.on_incoming(msg) {
                                    OnIncomingMessageOutcome::Ok => {
                                        // handled successfully
                                        progress = true;
                                    }
                                    OnIncomingMessageOutcome::BadMessage { error, message } => {
                                        error!(target: "net::session", ?error, msg=?message,  remote_peer_id=?this.remote_peer_id, "received invalid protocol message");
                                        this.close_on_error(error);
                                        return Poll::Ready(())
                                    }
                                    OnIncomingMessageOutcome::NoCapacity(msg) => {
                                        // failed to send due to lack of capacity
                                        this.pending_message_to_session = Some(msg);
                                        continue 'receive
                                    }
                                }
                            }
                            Err(err) => {
                                error!(target: "net::session", ?err, remote_peer_id=?this.remote_peer_id, "failed to receive message");
                                this.close_on_error(err);
                                return Poll::Ready(())
                            }
                        }
                    }
                }
            }

            if !progress {
                if this.internal_request_timeout_interval.poll_tick(cx).is_ready() {
                    let _ = this.internal_request_timeout_interval.poll_tick(cx);
                    // check for timed out requests
                    if this.check_timed_out_requests(Instant::now()) {
                        let _ = this.to_session.clone().try_send(
                            ActiveSessionMessage::ProtocolBreach { peer_id: this.remote_peer_id },
                        );
                    }
                }

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
    #[allow(unused)]
    received: Instant,
}

/// A request that waits for a response from the peer
pub(crate) struct InflightRequest {
    /// Request we sent to peer and the internal response channel
    request: RequestState,
    /// Instant when the request was sent
    timestamp: Instant,
    /// Time limit for the response
    deadline: Instant,
}

// === impl InflightRequest ===

impl InflightRequest {
    /// Returns true if the request is timedout
    #[inline]
    fn is_timed_out(&self, now: Instant) -> bool {
        now > self.deadline
    }

    /// Returns true if we're still waiting for a response
    #[inline]
    fn is_waiting(&self) -> bool {
        matches!(self.request, RequestState::Waiting(_))
    }

    fn timeout(&mut self) {
        let mut req = RequestState::TimedOut;
        std::mem::swap(&mut self.request, &mut req);

        if let RequestState::Waiting(req) = req {
            req.send_err_response(RequestError::Timeout);
        }
    }
}

/// All outcome variants when handling an incoming message
enum OnIncomingMessageOutcome {
    /// Message successfully handled.
    Ok,
    /// Message is considered to be in violation fo the protocol
    BadMessage { error: EthStreamError, message: EthMessage },
    /// Currently no capacity to handle the message
    NoCapacity(ActiveSessionMessage),
}

impl From<Result<(), ActiveSessionMessage>> for OnIncomingMessageOutcome {
    fn from(res: Result<(), ActiveSessionMessage>) -> Self {
        match res {
            Ok(_) => OnIncomingMessageOutcome::Ok,
            Err(msg) => OnIncomingMessageOutcome::NoCapacity(msg),
        }
    }
}

enum RequestState {
    /// Waiting for the response
    Waiting(PeerRequest),
    /// Request already timed out
    TimedOut,
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

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;
    use crate::session::{
        config::{INITIAL_REQUEST_TIMEOUT, PROTOCOL_BREACH_REQUEST_TIMEOUT},
        handle::PendingSessionEvent,
        start_pending_incoming_session,
    };
    use reth_ecies::util::pk2id;
    use reth_eth_wire::{
        GetBlockBodies, HelloMessage, Status, StatusBuilder, UnauthedEthStream, UnauthedP2PStream,
    };
    use reth_net_common::bandwidth_meter::BandwidthMeter;
    use reth_primitives::{ForkFilter, Hardfork, MAINNET};
    use secp256k1::{SecretKey, SECP256K1};
    use std::time::Duration;
    use tokio::{net::TcpListener, sync::mpsc};

    /// Returns a testing `HelloMessage` and new secretkey
    fn eth_hello(server_key: &SecretKey) -> HelloMessage {
        HelloMessage::builder(pk2id(&server_key.public_key(SECP256K1))).build()
    }

    struct SessionBuilder {
        remote_capabilities: Arc<Capabilities>,
        active_session_tx: mpsc::Sender<ActiveSessionMessage>,
        active_session_rx: ReceiverStream<ActiveSessionMessage>,
        to_sessions: Vec<mpsc::Sender<SessionCommand>>,
        secret_key: SecretKey,
        local_peer_id: PeerId,
        hello: HelloMessage,
        status: Status,
        fork_filter: ForkFilter,
        next_id: usize,
        bandwidth_meter: BandwidthMeter,
    }

    impl SessionBuilder {
        fn next_id(&mut self) -> SessionId {
            let id = self.next_id;
            self.next_id += 1;
            SessionId(id)
        }

        /// Connects a new Eth stream and executes the given closure with that established stream
        fn with_client_stream<F, O>(
            &self,
            local_addr: SocketAddr,
            f: F,
        ) -> Pin<Box<dyn Future<Output = ()> + Send>>
        where
            F: FnOnce(EthStream<P2PStream<ECIESStream<TcpStream>>>) -> O + Send + 'static,
            O: Future<Output = ()> + Send + Sync,
        {
            let status = self.status;
            let fork_filter = self.fork_filter.clone();
            let local_peer_id = self.local_peer_id;
            let mut hello = self.hello.clone();
            let key = SecretKey::new(&mut rand::thread_rng());
            hello.id = pk2id(&key.public_key(SECP256K1));
            Box::pin(async move {
                let outgoing = TcpStream::connect(local_addr).await.unwrap();
                let sink = ECIESStream::connect(outgoing, key, local_peer_id).await.unwrap();

                let (p2p_stream, _) = UnauthedP2PStream::new(sink).handshake(hello).await.unwrap();

                let (client_stream, _) = UnauthedEthStream::new(p2p_stream)
                    .handshake(status, fork_filter)
                    .await
                    .unwrap();
                f(client_stream).await
            })
        }

        async fn connect_incoming(&mut self, stream: TcpStream) -> ActiveSession {
            let remote_addr = stream.local_addr().unwrap();
            let session_id = self.next_id();
            let (_disconnect_tx, disconnect_rx) = oneshot::channel();
            let (pending_sessions_tx, pending_sessions_rx) = mpsc::channel(1);
            let metered_stream =
                MeteredStream::new_with_meter(stream, self.bandwidth_meter.clone());

            tokio::task::spawn(start_pending_incoming_session(
                disconnect_rx,
                session_id,
                metered_stream,
                pending_sessions_tx,
                remote_addr,
                self.secret_key,
                self.hello.clone(),
                self.status,
                self.fork_filter.clone(),
            ));

            let mut stream = ReceiverStream::new(pending_sessions_rx);

            match stream.next().await.unwrap() {
                PendingSessionEvent::Established {
                    session_id,
                    remote_addr,
                    peer_id,
                    capabilities,
                    conn,
                    ..
                } => {
                    let (_to_session_tx, messages_rx) = mpsc::channel(10);
                    let (commands_to_session, commands_rx) = mpsc::channel(10);

                    self.to_sessions.push(commands_to_session);

                    ActiveSession {
                        next_id: 0,
                        remote_peer_id: peer_id,
                        remote_addr,
                        remote_capabilities: Arc::clone(&capabilities),
                        session_id,
                        commands_rx: ReceiverStream::new(commands_rx),
                        to_session: MeteredSender::new(
                            self.active_session_tx.clone(),
                            "network_active_session",
                        ),
                        pending_message_to_session: None,
                        internal_request_tx: ReceiverStream::new(messages_rx).fuse(),
                        inflight_requests: Default::default(),
                        conn,
                        queued_outgoing: Default::default(),
                        received_requests_from_remote: Default::default(),
                        internal_request_timeout_interval: tokio::time::interval(
                            INITIAL_REQUEST_TIMEOUT,
                        ),
                        internal_request_timeout: Arc::new(AtomicU64::new(
                            INITIAL_REQUEST_TIMEOUT.as_millis() as u64,
                        )),
                        protocol_breach_request_timeout: PROTOCOL_BREACH_REQUEST_TIMEOUT,
                    }
                }
                ev => {
                    panic!("unexpected message {ev:?}")
                }
            }
        }
    }

    impl Default for SessionBuilder {
        fn default() -> Self {
            let (active_session_tx, active_session_rx) = mpsc::channel(100);

            let (secret_key, pk) = SECP256K1.generate_keypair(&mut rand::thread_rng());
            let local_peer_id = pk2id(&pk);

            Self {
                next_id: 0,
                remote_capabilities: Arc::new(Capabilities::from(vec![])),
                active_session_tx,
                active_session_rx: ReceiverStream::new(active_session_rx),
                to_sessions: vec![],
                hello: eth_hello(&secret_key),
                secret_key,
                local_peer_id,
                status: StatusBuilder::default().build(),
                fork_filter: Hardfork::Frontier
                    .fork_filter(&MAINNET)
                    .expect("The Frontier fork filter should exist on mainnet"),
                bandwidth_meter: BandwidthMeter::default(),
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_disconnect() {
        let mut builder = SessionBuilder::default();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let expected_disconnect = DisconnectReason::UselessPeer;

        let fut = builder.with_client_stream(local_addr, move |mut client_stream| async move {
            let msg = client_stream.next().await.unwrap().unwrap_err();
            assert_eq!(msg.as_disconnected().unwrap(), expected_disconnect);
        });

        tokio::task::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let mut session = builder.connect_incoming(incoming).await;

            session.start_disconnect(expected_disconnect).unwrap();
            session.await
        });

        fut.await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn handle_dropped_stream() {
        let mut builder = SessionBuilder::default();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let fut = builder.with_client_stream(local_addr, move |client_stream| async move {
            drop(client_stream);
            tokio::time::sleep(Duration::from_secs(1)).await
        });

        let (tx, rx) = oneshot::channel();

        tokio::task::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let session = builder.connect_incoming(incoming).await;
            session.await;

            tx.send(()).unwrap();
        });

        tokio::task::spawn(fut);

        rx.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_many_messages() {
        reth_tracing::init_test_tracing();
        let mut builder = SessionBuilder::default();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let num_messages = 100;

        let fut = builder.with_client_stream(local_addr, move |mut client_stream| async move {
            for _ in 0..num_messages {
                client_stream
                    .send(EthMessage::NewPooledTransactionHashes66(Vec::new().into()))
                    .await
                    .unwrap();
            }
        });

        let (tx, rx) = oneshot::channel();

        tokio::task::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let session = builder.connect_incoming(incoming).await;
            session.await;

            tx.send(()).unwrap();
        });

        tokio::task::spawn(fut);

        rx.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_request_timeout() {
        reth_tracing::init_test_tracing();

        let mut builder = SessionBuilder::default();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let request_timeout = Duration::from_millis(100);
        let drop_timeout = Duration::from_millis(1500);

        let fut = builder.with_client_stream(local_addr, move |client_stream| async move {
            let _client_stream = client_stream;
            tokio::time::sleep(drop_timeout * 60).await;
        });
        tokio::task::spawn(fut);

        let (incoming, _) = listener.accept().await.unwrap();
        let mut session = builder.connect_incoming(incoming).await;
        session
            .internal_request_timeout
            .store(request_timeout.as_millis() as u64, Ordering::Relaxed);
        session.protocol_breach_request_timeout = drop_timeout;
        session.internal_request_timeout_interval =
            tokio::time::interval_at(tokio::time::Instant::now(), request_timeout);
        let (tx, rx) = oneshot::channel();
        let req = PeerRequest::GetBlockBodies { request: GetBlockBodies(vec![]), response: tx };
        session.on_internal_peer_request(req, Instant::now());
        tokio::spawn(session);

        let err = rx.await.unwrap().unwrap_err();
        assert_eq!(err, RequestError::Timeout);

        // wait for protocol breach error
        let msg = builder.active_session_rx.next().await.unwrap();
        match msg {
            ActiveSessionMessage::ProtocolBreach { .. } => {}
            ev => unreachable!("{ev:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_keep_alive() {
        let mut builder = SessionBuilder::default();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let fut = builder.with_client_stream(local_addr, move |mut client_stream| async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), client_stream.next()).await;
            client_stream.into_inner().disconnect(DisconnectReason::UselessPeer).await.unwrap();
        });

        let (tx, rx) = oneshot::channel();

        tokio::task::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let session = builder.connect_incoming(incoming).await;
            session.await;

            tx.send(()).unwrap();
        });

        tokio::task::spawn(fut);

        rx.await.unwrap();
    }

    // This tests that incoming messages are delivered when there's capacity.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_at_capacity() {
        let mut builder = SessionBuilder::default();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let fut = builder.with_client_stream(local_addr, move |mut client_stream| async move {
            client_stream
                .send(EthMessage::NewPooledTransactionHashes68(Default::default()))
                .await
                .unwrap();
            let _ = tokio::time::timeout(Duration::from_secs(100), client_stream.next()).await;
        });
        tokio::task::spawn(fut);

        let (incoming, _) = listener.accept().await.unwrap();
        let session = builder.connect_incoming(incoming).await;

        // fill the entire message buffer with an unrelated message
        let mut num_fill_messages = 0;
        loop {
            if builder
                .active_session_tx
                .try_send(ActiveSessionMessage::ProtocolBreach { peer_id: PeerId::random() })
                .is_err()
            {
                break
            }
            num_fill_messages += 1;
        }

        tokio::task::spawn(async move {
            session.await;
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        for _ in 0..num_fill_messages {
            let message = builder.active_session_rx.next().await.unwrap();
            match message {
                ActiveSessionMessage::ProtocolBreach { .. } => {}
                ev => unreachable!("{ev:?}"),
            }
        }

        let message = builder.active_session_rx.next().await.unwrap();
        match message {
            ActiveSessionMessage::ValidMessage {
                message: PeerMessage::PooledTransactions(_),
                ..
            } => {}
            _ => unreachable!(),
        }
    }

    #[test]
    fn timeout_calculation_sanity_tests() {
        let rtt = Duration::from_secs(5);
        // timeout for an RTT of `rtt`
        let timeout = rtt * TIMEOUT_SCALING;

        // if rtt hasn't changed, timeout shouldn't change
        assert_eq!(calculate_new_timeout(timeout, rtt), timeout);

        // if rtt changed, the new timeout should change less than it
        assert!(calculate_new_timeout(timeout, rtt / 2) < timeout);
        assert!(calculate_new_timeout(timeout, rtt / 2) > timeout / 2);
        assert!(calculate_new_timeout(timeout, rtt * 2) > timeout);
        assert!(calculate_new_timeout(timeout, rtt * 2) < timeout * 2);
    }
}
