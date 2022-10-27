//! Support for handling peer sessions.
use crate::{
    session::handle::{
        ActiveSessionHandle, ActiveSessionMessage, PendingSessionEvent, PendingSessionHandle,
    },
    NodeId,
};
use fnv::FnvHashMap;
use futures::StreamExt;
use std::{
    future::Future,
    net::SocketAddr,
    task::{Context, Poll},
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_stream::wrappers::ReceiverStream;

mod handle;

/// Internal identifier for active sessions.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Hash)]
pub struct SessionId(usize);

/// Manages a set of sessions.
#[must_use = "Session Manager must be polled to process session events."]
pub(crate) struct SessionManager {
    /// Tracks the identifier for the next session.
    next_id: usize,
    /// All spawned session tasks.
    ///
    /// Note: If dropped, the session tasks are aborted.
    spawned_tasks: JoinSet<()>,
    /// All pending session that are currently handshaking, exchanging `Hello`s.
    ///
    /// Events produced during the authentication phase are reported to this manager. Once the
    /// session is authenticated, it can be moved to the `active_session` set.
    pending_sessions: FnvHashMap<SessionId, PendingSessionHandle>,
    /// All active sessions that are ready to exchange messages.
    active_sessions: FnvHashMap<NodeId, ActiveSessionHandle>,
    /// The original Sender half of the [`PendingSessionEvent`] channel.
    ///
    /// When a new (pending) session is created, the corresponding [`PendingSessionHandle`] will
    /// get a clone of this sender half.
    pending_sessions_tx: mpsc::Sender<PendingSessionEvent>,
    /// Receiver half that listens for [`PendingSessionEvent`] produced by pending sessions.
    pending_session_rx: ReceiverStream<PendingSessionEvent>,
    /// The original Sender half of the [`ActiveSessionEvent`] channel.
    ///
    /// When active session state is reached, the corresponding [`ActiveSessionHandle`] will get a
    /// clone of this sender half.
    active_session_tx: mpsc::Sender<ActiveSessionMessage>,
    /// Receiver half that listens for [`ActiveSessionEvent`] produced by pending sessions.
    active_session_rx: ReceiverStream<ActiveSessionMessage>,
    // TODO(mattsse): since new blocks need to be relayed to all nodes, this could either happen
    // through a separate `tokio::sync::broadcast::Sender` or part of the mpsc channels
}

// === impl SessionManager ===

impl SessionManager {
    /// Returns the next unique [`SessionId`].
    fn next_id(&mut self) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;
        SessionId(id)
    }

    /// Spawns the given future onto a new task that is tracked in the `spawned_tasks` [`JoinSet`].
    fn spawn<F>(&mut self, f: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawned_tasks.spawn(async move { f.await });
    }

    /// A incoming TCP connection was received. This starts the authentication process to turn this
    /// stream into an active peer session.
    ///
    /// Returns an error if the configured limit has been reached.
    pub(crate) fn on_incoming(
        &mut self,
        stream: TcpStream,
        remote_addr: SocketAddr,
    ) -> Result<SessionId, ExceedsSessionLimit> {
        // TODO spawn the pending task and track handle
        todo!()
    }

    /// This polls all the session handles and returns [`SessionEvent`].
    ///
    /// Active sessions are prioritized.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<SessionEvent> {
        // Poll events from active sessions
        match self.active_session_rx.poll_next_unpin(cx) {
            Poll::Pending => {}
            Poll::Ready(None) => {
                unreachable!("Manager holds both channel halves.")
            }
            Poll::Ready(Some(event)) => {}
        }

        // Poll the pending session event stream
        loop {
            let event = match self.pending_session_rx.poll_next_unpin(cx) {
                Poll::Pending => break,
                Poll::Ready(None) => unreachable!("Manager holds both channel halves."),
                Poll::Ready(Some(event)) => event,
            };

            match event {
                _ => {}
            }
        }

        Poll::Pending
    }
}

/// Events produced by the [`SessionManager`]
pub(crate) enum SessionEvent {}

/// The error thrown when the max configured limit has been reached and no more connections are
/// accepted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Session limit reached {0}")]
pub struct ExceedsSessionLimit(usize);

/// Starts the authentication process for a connection initiated by a remote peer.
async fn start_pending_incoming_session(
    session_id: SessionId,
    stream: TcpStream,
    disconnect_rx: oneshot::Receiver<()>,
    mut events: mpsc::Sender<PendingSessionEvent>,
) {
    // Authenticates the stream, sends `PendingSessionEvent` updates back, sends Disconnect if
    // disconnect trigger was fired.
    todo!()
}
