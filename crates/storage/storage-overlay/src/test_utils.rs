//! Helpers shared by the tests of this crate.

use crate::OverlayManager;
use reth_chain_state::{
    test_utils::test_canonical_in_memory_state_writer, CanonicalInMemoryStateWriter, InMemoryState,
};

/// Creates an overlay manager over a new in-memory state, and the writer of a canonical state over
/// the blocks of that state.
pub(crate) fn overlay_manager_with_writer() -> (OverlayManager, CanonicalInMemoryStateWriter) {
    let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
    (
        OverlayManager::new(in_memory_state),
        test_canonical_in_memory_state_writer(in_memory_state_writer),
    )
}
