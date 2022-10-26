//! Support for handling peer sessions.
mod handle;


/// Internal identifier for active sessions.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Hash)]
pub(crate) struct SessionId(usize);

/// Manages a set of sessions.
pub(crate) struct SessionManager {
    /// Tracks the identifier for the next session.
    next_id: usize,

}


// === impl Connections ===

impl SessionManager {

    /// Returns the next unique [`ConnectionId`].
    fn next_id(&mut self) -> SessionId {
        let id = self.next_id;
        self.next_id+=1;
        SessionId(id)
    }
}