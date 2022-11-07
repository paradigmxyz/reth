//! Represents an established session.

use crate::{
    session::{
        handle::{ActiveSessionMessage, SessionCommand},
        SessionId,
    },
    NodeId,
};

use fnv::FnvHashMap;
use futures::{stream::Fuse, Sink, Stream};
use pin_project::pin_project;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::capability::{Capabilities, CapabilityMessage};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{net::TcpStream, sync::mpsc};
use tokio_stream::wrappers::ReceiverStream;

/// The type that advances an established session by listening for incoming messages (from local
/// node or read from connection) and emitting events back to the [`SessionHandler`].
#[pin_project]
pub(crate) struct ActiveSession {
    /// The underlying connection.
    #[pin]
    pub(crate) conn: ECIESStream<TcpStream>,
    /// Identifier of the node we're connected to.
    pub(crate) remote_node_id: NodeId,
    /// All capabilities the peer announced
    pub(crate) remote_capabilities: Arc<Capabilities>,
    /// Internal identifier of this session
    pub(crate) session_id: SessionId,
    /// Incoming commands from the manager
    #[pin]
    pub(crate) commands_rx: ReceiverStream<SessionCommand>,
    /// Sink to send messages to the [`SessionManager`].
    pub(crate) to_session: mpsc::Sender<ActiveSessionMessage>,
    /// Incoming request to send to delegate to the remote peer.
    #[pin]
    pub(crate) messages_rx: Fuse<ReceiverStream<CapabilityMessage>>,
    /// All requests currently in progress.
    pub(crate) inflight_requests: FnvHashMap<u64, ()>,
    /// Buffered messages that should be sent to the remote peer.
    pub(crate) buffered_outgoing: VecDeque<CapabilityMessage>,
}

impl Future for ActiveSession {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            let mut progress = false;
            // we prioritize incoming messages
            loop {
                match this.commands_rx.as_mut().poll_next(cx) {
                    Poll::Pending => break,
                    Poll::Ready(None) => {
                        // this is only possible when the manager was dropped, in which case we also
                        // terminate this session
                        return Poll::Ready(())
                    }
                    Poll::Ready(Some(_cmd)) => {
                        progress = true;
                        // TODO handle command

                        continue
                    }
                }
            }

            while let Poll::Ready(Some(_msg)) = this.messages_rx.as_mut().poll_next(cx) {
                progress = true;
                // TODO handle request
            }

            // send and flush
            while this.conn.as_mut().poll_ready(cx).is_ready() {
                if let Some(_msg) = this.buffered_outgoing.pop_front() {
                    progress = true;
                    // TODO encode message and start send
                } else {
                    break
                }
            }

            loop {
                match this.conn.as_mut().poll_next(cx) {
                    Poll::Pending => break,
                    Poll::Ready(None) => {
                        // disconnected
                    }
                    Poll::Ready(Some(_msg)) => {
                        progress = true;
                        // decode and handle message

                        continue
                    }
                }
            }

            if !progress {
                return Poll::Pending
            }
        }
    }
}
