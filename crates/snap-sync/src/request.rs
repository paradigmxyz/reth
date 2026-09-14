//! Shared request identity and peer selection for state downloads.

use crate::SnapSyncError;
use reth_network_p2p::{priority::Priority, snap::client::SnapRequestOptions};
use reth_network_peers::PeerId;
use reth_tasks::Runtime;

/// Request state shared by the account, storage and bytecode downloaders.
#[derive(Debug)]
pub(crate) struct SnapRequests<'a, C> {
    // A reference avoids requiring network clients to implement Clone.
    pub(crate) client: &'a C,
    // Proof verification must stay off the async worker.
    pub(crate) runtime: Runtime,
    // One sequence covers follow-ups across every state domain.
    pub(crate) request_id: u64,
}

impl<'a, C> SnapRequests<'a, C> {
    pub(crate) const fn new(client: &'a C, runtime: Runtime) -> Self {
        Self { client, runtime, request_id: 0 }
    }

    // Failing on wrap prevents a stale response from matching a new logical request.
    pub(crate) fn next_id(&mut self) -> Result<u64, SnapSyncError> {
        self.request_id = self.request_id.checked_add(1).ok_or_else(|| {
            SnapSyncError::InvalidRequest("snap request id space exhausted".to_string())
        })?;
        Ok(self.request_id)
    }
}

// Prioritizes follow-ups while preserving the unavailable-peer set.
pub(crate) fn request_options(excluded: &[PeerId]) -> SnapRequestOptions {
    let priority = if excluded.is_empty() { Priority::Normal } else { Priority::High };
    SnapRequestOptions::new(priority).with_excluded_peers(excluded.iter().copied())
}

// Keeps exclusions stable and duplicate-free across retries.
pub(crate) fn push_peer(peers: &mut Vec<PeerId>, peer_id: PeerId) {
    if !peers.contains(&peer_id) {
        peers.push(peer_id);
    }
}
