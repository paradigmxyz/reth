//! Trie changeset computation utilities.
//!
//! This module provides functionality to compute trie changesets for a given block,
//! which represent the old trie node values before the block was processed.

use crate::{DatabaseHashedPostState, DatabaseStateRoot, DatabaseTrieCursorFactory};
use alloy_primitives::BlockNumber;
use reth_storage_api::{BlockNumReader, ChangeSetReader, DBProvider, StageCheckpointReader};
use reth_storage_errors::provider::ProviderError;
use reth_trie::{
    changesets::compute_trie_changesets, trie_cursor::InMemoryTrieCursorFactory,
    HashedPostStateSorted, KeccakKeyHasher, StateRoot, TrieInputSorted,
};
use reth_trie_common::updates::TrieUpdatesSorted;
use std::sync::Arc;

/// Computes trie changesets for a block.
///
/// # Algorithm
///
/// For block N:
/// 1. Query cumulative `HashedPostState` revert for block N-1 (from db tip to after N-1)
/// 2. Use that to calculate cumulative `TrieUpdates` revert for block N-1
/// 3. Query per-block `HashedPostState` revert for block N
/// 4. Create prefix sets from the per-block revert (step 3)
/// 5. Create overlay with cumulative trie updates and cumulative state revert for N-1
/// 6. Calculate trie updates for block N using the overlay and per-block `HashedPostState`.
/// 7. Compute changesets using the N-1 overlay and the newly calculated trie updates for N
///
/// # Arguments
///
/// * `provider` - Database provider with changeset access
/// * `block_number` - Block number to compute changesets for
///
/// # Returns
///
/// Changesets (old trie node values) for the specified block
///
/// # Errors
///
/// Returns error if:
/// - Block number exceeds database tip (based on Finish stage checkpoint)
/// - Database access fails
/// - State root computation fails
pub fn compute_block_trie_changesets<Provider>(
    provider: &Provider,
    block_number: BlockNumber,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: DBProvider + StageCheckpointReader + ChangeSetReader + BlockNumReader,
{
    // Step 1: Collect/calculate state reverts

    // This is just the changes from this specific block
    let individual_state_revert = HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(
        provider,
        block_number..=block_number,
    )?;

    // This reverts all changes from db tip back to just after block was processed
    let cumulative_state_revert =
        HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(provider, (block_number + 1)..)?;

    // This reverts all changes from db tip back to just after block-1 was processed
    let mut cumulative_state_revert_prev = cumulative_state_revert.clone();
    cumulative_state_revert_prev.extend_ref(&individual_state_revert);

    // Step 2: Calculate cumulative trie updates revert for block-1
    // This gives us the trie state as it was after block-1 was processed
    let prefix_sets_prev = cumulative_state_revert_prev.construct_prefix_sets();
    let input_prev = TrieInputSorted::new(
        Arc::default(),
        Arc::new(cumulative_state_revert_prev),
        prefix_sets_prev,
    );

    let cumulative_trie_updates_prev =
        StateRoot::overlay_root_from_nodes_with_updates(provider.tx_ref(), input_prev)
            .map_err(ProviderError::other)?
            .1
            .into_sorted();

    // Step 2: Create prefix sets from individual revert (only paths changed by this block)
    let prefix_sets = individual_state_revert.construct_prefix_sets();

    // Step 3: Calculate trie updates for block
    // Use cumulative trie updates for block-1 as the node overlay and cumulative state for block
    let input = TrieInputSorted::new(
        Arc::new(cumulative_trie_updates_prev.clone()),
        Arc::new(cumulative_state_revert),
        prefix_sets,
    );

    let trie_updates = StateRoot::overlay_root_from_nodes_with_updates(provider.tx_ref(), input)
        .map_err(ProviderError::other)?
        .1
        .into_sorted();

    // Step 4: Compute changesets using cumulative trie updates for block-1 as overlay
    // Create an overlay cursor factory that has the trie state from after block-1
    let db_cursor_factory = DatabaseTrieCursorFactory::new(provider.tx_ref());
    let overlay_factory =
        InMemoryTrieCursorFactory::new(db_cursor_factory, &cumulative_trie_updates_prev);

    let changesets =
        compute_trie_changesets(&overlay_factory, &trie_updates).map_err(ProviderError::other)?;

    Ok(changesets)
}
