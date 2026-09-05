//! Database-backed trie changeset computation utilities.
//!
//! This module reconstructs trie changesets from database state. The resulting changesets contain
//! the old trie node values needed to revert a block or contiguous range of blocks.

use crate::DatabaseHashedPostState;
use alloy_primitives::BlockNumber;
use reth_storage_api::{BlockNumReader, ChangeSetReader, StorageChangeSetReader};
use reth_storage_errors::provider::ProviderError;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    StateRoot,
};
use reth_trie_common::updates::TrieUpdatesSorted;
use std::ops::RangeInclusive;
use tracing::debug;

/// Computes trie changesets for a block.
///
/// For block `N`, this reconstructs the trie as it existed after `N`, then calculates the trie
/// updates needed to restore the state before `N`.
///
/// # Errors
///
/// Returns an error if the block exceeds the database tip, database access fails, or state root
/// computation fails.
pub fn compute_block_trie_changesets<Provider, StateTrieProvider>(
    provider: &Provider,
    state_trie_provider: &StateTrieProvider,
    block_number: BlockNumber,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader,
    StateTrieProvider: TrieCursorFactory + HashedCursorFactory,
{
    let db_tip_block = provider.best_block_number()?;
    compute_range_trie_changesets(
        provider,
        state_trie_provider,
        block_number..=block_number,
        db_tip_block,
    )
}

/// Computes aggregate trie changesets for an inclusive block range.
///
/// The returned changesets restore the trie from the state after `range.end()` to the state before
/// `range.start()`. `db_tip_block` must be the current database tip for `provider`.
///
/// # Errors
///
/// Returns an error if the range exceeds `db_tip_block`, database access fails, or state root
/// computation fails.
pub fn compute_range_trie_changesets<Provider, StateTrieProvider>(
    provider: &Provider,
    state_trie_provider: &StateTrieProvider,
    range: RangeInclusive<BlockNumber>,
    db_tip_block: BlockNumber,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader,
    StateTrieProvider: TrieCursorFactory + HashedCursorFactory,
{
    let start_block = *range.start();
    let end_block = *range.end();

    if start_block > end_block {
        return Ok(TrieUpdatesSorted::default())
    }

    if end_block > db_tip_block {
        return Err(ProviderError::InsufficientChangesets {
            requested: end_block,
            available: 0..=db_tip_block,
        })
    }

    debug!(
        target: "trie::changesets",
        start_block,
        end_block,
        db_tip_block,
        "Computing range trie changesets from database state"
    );

    // Collect the state revert for the requested range.
    let range_state_revert = reth_trie::HashedPostStateSorted::from_reverts(provider, range)?;
    let range_prefix_sets = range_state_revert.construct_prefix_sets();

    let (range_nodes, range_state) = if end_block == db_tip_block {
        debug!(
            target: "trie::changesets",
            start_block,
            end_block,
            db_tip_block,
            "Skipping tail trie revert computation for tip-ended range"
        );

        (TrieUpdatesSorted::default(), range_state_revert)
    } else {
        // Collect the state revert from the database tip to just after the range.
        let tail_state_revert = end_block
            .checked_add(1)
            .map(|next_block| {
                reth_trie::HashedPostStateSorted::from_reverts(provider, next_block..)
            })
            .transpose()?
            .unwrap_or_default();

        // Compute trie reverts from the database tip to just after the range.
        let tail_prefix_sets = tail_state_revert.construct_prefix_sets().freeze();
        let tail_trie_revert = StateRoot::new(
            state_trie_provider,
            HashedPostStateCursorFactory::new(state_trie_provider, &tail_state_revert),
        )
        .with_prefix_sets(tail_prefix_sets)
        .root_with_updates()
        .map_err(ProviderError::other)?
        .1
        .into_sorted();

        // Overlay the post-range trie and compute the trie revert to the pre-range state.
        let mut pre_range_state_revert = tail_state_revert;
        pre_range_state_revert.extend_ref_and_sort(&range_state_revert);

        (tail_trie_revert, pre_range_state_revert)
    };

    let range_trie_revert = StateRoot::new(
        InMemoryTrieCursorFactory::new(state_trie_provider, &range_nodes),
        HashedPostStateCursorFactory::new(state_trie_provider, &range_state),
    )
    .with_prefix_sets(range_prefix_sets.freeze())
    .root_with_updates()
    .map_err(ProviderError::other)?
    .1
    .into_sorted();

    debug!(
        target: "trie::changesets",
        start_block,
        end_block,
        num_account_nodes = range_trie_revert.account_nodes_ref().len(),
        num_storage_tries = range_trie_revert.storage_tries_ref().len(),
        "Computed range trie changesets successfully"
    );

    Ok(range_trie_revert)
}
