//! Helpers shared by the tests of this crate.

use crate::OverlayManager;
use reth_chain_state::{
    test_utils::test_canonical_in_memory_state_writer, CanonicalInMemoryStateWriter,
};

/// Creates the writer of a canonical state over the blocks `manager` builds overlays from.
pub(crate) fn canonical_in_memory_state_writer_for(
    manager: &OverlayManager,
) -> CanonicalInMemoryStateWriter {
    test_canonical_in_memory_state_writer(manager.in_memory_state().clone())
}
