//! Helpers shared by the tests of this crate.

use crate::OverlayManager;
use reth_chain_state::{CanonicalInMemoryState, ChainInfoHeaders};
use reth_primitives_traits::SealedHeader;

/// Creates the canonical state that updates the blocks `manager` builds overlays from.
pub(crate) fn canonical_state_for(manager: &OverlayManager) -> CanonicalInMemoryState {
    CanonicalInMemoryState::with_head(
        ChainInfoHeaders { canonical_head: SealedHeader::default(), safe: None, finalized: None },
        manager.in_memory_state().clone(),
    )
}
