//! Helpers shared by the tests of this crate.

use crate::OverlayManager;
use reth_chain_state::{CanonicalInMemoryStateWriter, ChainInfoHeaders};
use reth_primitives_traits::SealedHeader;

/// Creates the writer of a canonical state over the blocks `manager` builds overlays from.
pub(crate) fn canonical_state_writer_for(manager: &OverlayManager) -> CanonicalInMemoryStateWriter {
    CanonicalInMemoryStateWriter::new(
        ChainInfoHeaders { canonical_head: SealedHeader::default(), safe: None, finalized: None },
        manager.in_memory_state().clone(),
    )
}
