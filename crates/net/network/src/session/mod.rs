//! Support for handling peer sessions.
use crate::{
    session::handle::{
        ActiveSessionHandle, ActiveSessionMessage, PendingSessionEvent, PendingSessionHandle,
    },
    PeerId,
};
use fnv::FnvHashMap;
use tokio::sync::mpsc;

mod handle;

/// Manages a set of sessions.
pub(crate) struct SessionManager {
    /// Tracks the identifier for the next session.
    next_id: usize,
    /// All pending session that are currently handshaking, exchanging `Hello`s.
    ///
    /// Events produced during the authentication phase are reported to this manager. Once the
    /// session is authenticated, it can be moved to the `active_session` set.
    pending_sessions: FnvHashMap<SessionId, PendingSessionHandle>,
    /// All active sessions that are ready to exchange messages.
    active_sessions: FnvHashMap<PeerId, ActiveSessionHandle>,
    /// The original Sender half of the [`PendingSessionEvent`] channel.
    ///
    /// When a new (pending) session is created, the corresponding [`PendingSessionHandle`] will
    /// get a clone of this sender half.
    pending_sessions_tx: mpsc::Sender<PendingSessionEvent>,
    /// Receiver half that listens for [`PendingSessionEvent`] produced by pending sessions.
    pending_session_rx: mpsc::Receiver<PendingSessionEvent>,
    /// The original Sender half of the [`ActiveSessionEvent`] channel.
    ///
    /// When active session state is reached, the corresponding [`ActiveSessionHandle`] will get a
    /// clone of this sender half.
    active_session_tx: mpsc::Sender<ActiveSessionMessage>,
    /// Receiver half that listens for [`ActiveSessionEvent`] produced by pending sessions.
    active_session_rx: mpsc::Receiver<ActiveSessionMessage>,
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
}

/// Internal identifier for active sessions.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Hash)]
pub(crate) struct SessionId(usize);
