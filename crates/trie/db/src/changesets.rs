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
/// The returned changesets restore the trie from the state after `range.end()` to the state before
/// `range.start()`. They contain every trie path changed within the range, including nodes created
/// in one block and removed in a later block of the range. An empty range returns empty changesets.
///
/// `state_trie_provider` must expose the trie and hashed state at `db_tip_block`.
///
/// `forward_updates` holds original trie updates of executed blocks on the chain ending at
/// `db_tip_block`, keyed by block number in strictly increasing order. Any subset of blocks may be
/// supplied, including none; supplied blocks speed up the computation without affecting the
/// correctness of the result.
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

    // Revert blocks after the range to reconstruct the trie at the end of the range.
    let mut overlay = TrieInputSorted::default();
    revert_trie_range(
        provider,
        state_trie_provider,
        forward_updates,
        (end_block + 1)..=db_tip_block,
        &mut overlay,
        |_| {},
    )?;

    // Revert the range itself, collecting its changesets with older values taking precedence.
    let mut changesets = TrieUpdatesSorted::default();
    revert_trie_range(
        provider,
        state_trie_provider,
        forward_updates,
        range,
        &mut overlay,
        |segment_changesets| changesets.extend_ref_and_sort(segment_changesets),
    )?;

    debug!(
        target: "trie::changesets",
        start_block,
        end_block,
        num_account_nodes = changesets.account_nodes_ref().len(),
        num_storage_tries = changesets.storage_tries_ref().len(),
        "Computed range trie changesets successfully"
    );

    Ok(changesets)
}

/// Reverts `overlay` across `range`, newest block first, passing the trie changesets of each
/// reverted segment to `on_segment`.
///
/// On entry `overlay` must hold the hashed state and trie reverts that, layered over
/// `state_trie_provider`, describe the state after `range.end()`. On return it describes the state
/// before `range.start()`.
fn revert_trie_range<Provider, StateTrieProvider>(
    provider: &Provider,
    state_trie_provider: &StateTrieProvider,
    forward_updates: &[(BlockNumber, Arc<TrieUpdatesSorted>)],
    range: RangeInclusive<BlockNumber>,
    overlay: &mut TrieInputSorted,
    mut on_segment: impl FnMut(&TrieUpdatesSorted),
) -> Result<(), ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader,
    StateTrieProvider: TrieCursorFactory + HashedCursorFactory,
{
    if range.is_empty() {
        return Ok(())
    }

    let mut end = *range.end();
    loop {
        // Extend the segment backwards over consecutive blocks with forward updates. A block
        // without forward updates forms its own segment, because a calculation spanning several
        // blocks cannot see nodes that are created and removed between its endpoints.
        let mut start = end;
        let mut forward_indices = 0..0;
        if let Ok(index) = forward_updates.binary_search_by_key(&end, |(block, _)| *block) {
            forward_indices = index..index + 1;
            while start > *range.start() &&
                forward_indices.start > 0 &&
                forward_updates[forward_indices.start - 1].0 == start - 1
            {
                forward_indices.start -= 1;
                start -= 1;
            }
        }

        // Calculate the segment's trie changesets from its state reverts.
        let segment_state_reverts = HashedPostStateSorted::from_reverts(provider, start..=end)?;
        let prefixes = segment_state_reverts.construct_prefix_sets().freeze();
        Arc::make_mut(&mut overlay.state).extend_ref_and_sort(&segment_state_reverts);

        let calculated_changesets = StateRoot::new(
            InMemoryTrieCursorFactory::new(state_trie_provider, overlay.nodes.as_ref()),
            HashedPostStateCursorFactory::new(state_trie_provider, overlay.state.as_ref()),
        )
        .with_prefix_sets(prefixes)
        .root_with_updates()?
        .1
        .into_sorted();

        // The calculation omits paths whose value is equal at both ends of the segment, such as a
        // node created and then removed within it. Consumers may apply the changesets to a trie
        // that still holds such a node, so every touched path must be present. For an omitted
        // path, the newest forward update holds the value at the end of the segment, which equals
        // its value before the segment. Calculated values take precedence.
        let segment_changesets = TrieUpdatesSorted::merge_slice(
            &once(&calculated_changesets)
                .chain(forward_updates[forward_indices].iter().rev().map(|(_, updates)| &**updates))
                .collect::<Vec<_>>(),
        );

        Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(&segment_changesets);
        on_segment(&segment_changesets);

        if start == *range.start() {
            return Ok(())
        }
        end = start - 1;
    }
}
