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
    HashedPostStateSorted, StateRoot,
};
use reth_trie_common::{updates::TrieUpdatesSorted, TrieInputSorted};
use std::{iter::once, ops::RangeInclusive, sync::Arc};
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
        &[],
        block_number..=block_number,
        db_tip_block,
    )
}

/// Computes aggregate trie changesets for an inclusive block range.
///
/// `state_trie_provider` must expose the complete trie and hashed state at `db_tip_block`.
/// `forward_updates` contains original executed-block trie updates on that same chain, sorted by
/// strictly increasing block number. Consecutive available blocks are reverted together, preserving
/// forward paths omitted by the aggregate calculation. Missing blocks are reverted individually so
/// transient nodes are retained.
/// Returns before-values for the requested range only. Later blocks are used internally to
/// reconstruct its starting state; their changesets are not included. Empty ranges return empty
/// changesets. `db_tip_block` must be the current database tip for `provider`.
///
/// # Errors
///
/// Returns an error if the range exceeds `db_tip_block`, database access fails, or state root
/// computation fails.
pub fn compute_range_trie_changesets<Provider, StateTrieProvider>(
    provider: &Provider,
    state_trie_provider: &StateTrieProvider,
    forward_updates: &[(BlockNumber, Arc<TrieUpdatesSorted>)],
    range: RangeInclusive<BlockNumber>,
    db_tip_block: BlockNumber,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader,
    StateTrieProvider: TrieCursorFactory + HashedCursorFactory,
{
    debug_assert!(
        forward_updates.is_sorted_by(|(a, _), (b, _)| a < b),
        "forward updates must have strictly increasing block numbers"
    );

    if range.is_empty() {
        return Ok(TrieUpdatesSorted::default())
    }

    let start_block = *range.start();
    let end_block = *range.end();

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

    let mut overlay = TrieInputSorted::default();
    // Rewind later blocks to reconstruct the trie at the end of the requested range.
    if end_block < db_tip_block {
        rewind_trie_range(
            provider,
            state_trie_provider,
            forward_updates,
            (end_block + 1)..=db_tip_block,
            &mut overlay,
        )?;
    }

    let reverts =
        rewind_trie_range(provider, state_trie_provider, forward_updates, range, &mut overlay)?;

    debug!(
        target: "trie::changesets",
        start_block,
        end_block,
        num_account_nodes = reverts.account_nodes_ref().len(),
        num_storage_tries = reverts.storage_tries_ref().len(),
        "Computed range trie changesets successfully"
    );

    Ok(reverts)
}

/// Rewinds the current state and trie views, returning only this range's changesets.
fn rewind_trie_range<Provider, StateTrieProvider>(
    provider: &Provider,
    state_trie_provider: &StateTrieProvider,
    forward_updates: &[(BlockNumber, Arc<TrieUpdatesSorted>)],
    blocks: RangeInclusive<BlockNumber>,
    overlay: &mut TrieInputSorted,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader,
    StateTrieProvider: TrieCursorFactory + HashedCursorFactory,
{
    let mut reverts = TrieUpdatesSorted::default();
    let mut end = *blocks.end();
    while end >= *blocks.start() {
        let mut start = end;
        let mut segment_range = 0..0;

        // Select updates of consecutive blocks iterating backwards from `end`.
        // Without the original updates, reverting one block at a time preserves nodes that
        // appear and disappear inside the range. An endpoint-only calculation would omit them.
        if let Ok(index) = forward_updates.binary_search_by_key(&end, |(block, _)| *block) {
            segment_range = index..index + 1;
            while segment_range.start > 0 && start > *blocks.start() {
                if forward_updates[segment_range.start - 1].0 != start - 1 {
                    break
                }
                segment_range.start -= 1;
                start -= 1;
            }
        }

        // Collect reverts for the segment and use them to generate the trie reverts for the
        // segment.
        let segment_state_reverts = HashedPostStateSorted::from_reverts(provider, start..=end)?;
        let prefixes = segment_state_reverts.construct_prefix_sets().freeze();
        Arc::make_mut(&mut overlay.state).extend_ref_and_sort(&segment_state_reverts);

        let segment_trie_reverts = StateRoot::new(
            InMemoryTrieCursorFactory::new(state_trie_provider, overlay.nodes.as_ref()),
            HashedPostStateCursorFactory::new(state_trie_provider, overlay.state.as_ref()),
        )
        .with_prefix_sets(prefixes)
        .root_with_updates()
        .map_err(ProviderError::other)?
        .1
        .into_sorted();

        // The first entry wins, so calculated target values precede newest-to-oldest forward
        // values.
        let segment_trie_forward =
            forward_updates[segment_range].iter().rev().map(|(_, updates)| updates.as_ref());
        let segment_trie_reverts =
            TrieUpdatesSorted::merge_iter(once(&segment_trie_reverts).chain(segment_trie_forward));

        Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(&segment_trie_reverts);
        reverts.extend_ref_and_sort(&segment_trie_reverts);

        if start == *blocks.start() {
            break
        }
        end = start - 1;
    }

    Ok(reverts)
}
