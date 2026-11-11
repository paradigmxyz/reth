//! Helpers shared by the tests of this crate.

use crate::OverlayManager;
use reth_chain_state::{InMemoryState, InMemoryStateWriter};

/// Creates an overlay manager for a new in-memory state, and the writer of that state.
pub(crate) fn overlay_manager_with_writer() -> (OverlayManager, InMemoryStateWriter) {
    let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
    (OverlayManager::new(in_memory_state), in_memory_state_writer)
}
