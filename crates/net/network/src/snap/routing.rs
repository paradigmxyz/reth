//! `snap/2` request-routing state owned by the [`SessionManager`](crate::session::SessionManager).

use crate::snap::{SnapClient, SnapPeerRequest};
use futures::StreamExt;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// `snap/2` request-routing state owned by the [`SessionManager`](crate::session::SessionManager).
///
/// Holds the channel every [`SnapClient`] sends requests on and the shared count of snap-capable
/// peers surfaced back to those clients.
#[derive(Debug)]
pub(crate) struct SnapRouting {
    /// Sender handed to every [`SnapClient`] to route `snap/2` requests into the manager.
    requests_tx: mpsc::UnboundedSender<SnapPeerRequest>,
    /// Receiver of `snap/2` requests from [`SnapClient`]s, routed to a snap-capable session.
    requests_rx: UnboundedReceiverStream<SnapPeerRequest>,
    /// Number of active sessions that negotiated `snap/2`, shared with every [`SnapClient`] for
    /// its connected-peer count.
    peer_count: Arc<AtomicUsize>,
}

impl SnapRouting {
    /// Creates empty routing state.
    pub(crate) fn new() -> Self {
        let (requests_tx, requests_rx) = mpsc::unbounded_channel();
        Self {
            requests_tx,
            requests_rx: UnboundedReceiverStream::new(requests_rx),
            peer_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns a [`SnapClient`] backed by this routing state.
    pub(crate) fn client(&self) -> SnapClient {
        SnapClient::new(self.requests_tx.clone(), Arc::clone(&self.peer_count))
    }

    /// Polls for the next queued client request.
    pub(crate) fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<SnapPeerRequest>> {
        self.requests_rx.poll_next_unpin(cx)
    }

    /// Records that a snap-capable session was added.
    pub(crate) fn on_session_added(&self) {
        self.peer_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records that a snap-capable session was removed.
    pub(crate) fn on_session_removed(&self) {
        self.peer_count.fetch_sub(1, Ordering::Relaxed);
    }
}
