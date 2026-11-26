//! Helpers shared by the tests of this crate.

use crate::{OverlayBuilder, OverlayManager, StateTrieOverlay};
use alloy_primitives::B256;
use reth_chain_state::{InMemoryState, InMemoryStateWriter};

/// Creates an overlay manager, a new in-memory state, and the writer of that state.
pub(crate) fn overlay_manager_with_writer() -> (OverlayManager, InMemoryState, InMemoryStateWriter)
{
    let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
    (OverlayManager::default(), in_memory_state, in_memory_state_writer)
}

/// Asserts that `actual` holds the same hashed accounts and account trie nodes as `expected`,
/// which must hold account trie nodes, so that the comparison covers them.
pub(crate) fn assert_same_overlay(actual: &StateTrieOverlay, expected: &StateTrieOverlay) {
    let (actual, expected) = (actual.input(), expected.input());
    assert!(
        !expected.nodes.account_nodes_ref().is_empty(),
        "nothing to compare account nodes with"
    );
    assert_eq!(actual.state.accounts, expected.state.accounts);
    assert_eq!(actual.nodes.account_nodes_ref(), expected.nodes.account_nodes_ref());
}

/// Builds an overlay against the blocks currently tracked by the test's in-memory state.
pub(crate) fn test_overlay_builder(
    manager: &OverlayManager,
    in_memory_state: &InMemoryState,
    parent_hash: B256,
) -> OverlayBuilder {
    OverlayBuilder::new(
        parent_hash,
        in_memory_state.snapshot(),
        in_memory_state.clone(),
        manager.clone(),
    )
}
