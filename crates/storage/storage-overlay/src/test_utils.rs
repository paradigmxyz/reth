//! Helpers shared by the tests of this crate.

use crate::{OverlayManager, StateTrieOverlay};
use reth_chain_state::{InMemoryState, InMemoryStateWriter};

/// Creates an overlay manager over a new in-memory state, and the writer of that state.
pub(crate) fn overlay_manager_with_writer() -> (OverlayManager, InMemoryStateWriter) {
    let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
    (OverlayManager::new(in_memory_state), in_memory_state_writer)
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
