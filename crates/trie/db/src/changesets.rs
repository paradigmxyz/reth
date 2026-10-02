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
use reth_trie_common::updates::TrieUpdatesSorted;
use std::{collections::BTreeMap, ops::RangeInclusive, sync::Arc};
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
        &BTreeMap::new(),
        block_number..=block_number,
        db_tip_block,
    )
    .map(|result| (*result.reverts).clone())
}

/// Computes aggregate trie changesets for an inclusive block range.
///
/// `state_trie_provider` must expose the complete trie and hashed state at `db_tip_block`.
/// `forward_updates` contains original executed-block trie updates on that same chain. Consecutive
/// available blocks are reverted together, preserving forward paths omitted by the aggregate
/// calculation. Missing blocks are reverted individually so transient nodes are retained.
/// The result applies at any trie frontier between the target and the range end (or database tip
/// for `overlay`). An empty range returns no range reverts, but still rewinds the tail.
/// `db_tip_block` must be the current database tip for `provider`.
///
/// # Errors
///
/// Returns an error if the range exceeds `db_tip_block`, database access fails, or state root
/// computation fails.
pub fn compute_range_trie_changesets<Provider, StateTrieProvider>(
    provider: &Provider,
    state_trie_provider: &StateTrieProvider,
    forward_updates: &BTreeMap<BlockNumber, Arc<TrieUpdatesSorted>>,
    range: RangeInclusive<BlockNumber>,
    db_tip_block: BlockNumber,
) -> Result<ComputedTrieChangesets, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader,
    StateTrieProvider: TrieCursorFactory + HashedCursorFactory,
{
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

    let mut state = HashedPostStateSorted::default();
    let mut overlay = TrieUpdatesSorted::default();
    let mut reverts = TrieUpdatesSorted::default();

    // Rewind the tail first, then the requested range, keeping the current trie and state views.
    let tail = end_block.checked_add(1).map(|start| start..=db_tip_block);
    for (is_range, blocks) in tail.into_iter().map(|tail| (false, tail)).chain([(true, range)]) {
        let mut next = (!blocks.is_empty()).then_some(*blocks.end());
        while let Some(end) = next {
            let mut start = end;
            let mut forward = Vec::new();
            if let Some(updates) = forward_updates.get(&end) {
                forward.push(updates);
                while start > *blocks.start() {
                    let Some(updates) = forward_updates.get(&(start - 1)) else { break };
                    forward.push(updates);
                    start -= 1;
                }
            }

            // Without the original updates, reverting one block at a time preserves nodes that
            // appear and disappear inside the range. An endpoint-only calculation would omit them.
            let segment_state = HashedPostStateSorted::from_reverts(provider, start..=end)?;
            let prefixes = segment_state.construct_prefix_sets().freeze();
            state.extend_ref_and_sort(&segment_state);
            let updates = StateRoot::new(
                InMemoryTrieCursorFactory::new(state_trie_provider, &overlay),
                HashedPostStateCursorFactory::new(state_trie_provider, &state),
            )
            .with_prefix_sets(prefixes)
            .root_with_updates()
            .map_err(ProviderError::other)?
            .1
            .into_sorted();

            // Newest forward values win; the calculated target values override them.
            let segment = if forward.is_empty() {
                updates
            } else {
                let mut segment = TrieUpdatesSorted::merge_slice(&forward);
                segment.extend_ref_and_sort(&updates);
                segment
            };
            overlay.extend_ref_and_sort(&segment);
            if is_range {
                reverts.extend_ref_and_sort(&segment);
            }
            next = (start > *blocks.start()).then(|| start - 1);
        }
    }

    debug!(
        target: "trie::changesets",
        start_block,
        end_block,
        num_account_nodes = reverts.account_nodes_ref().len(),
        num_storage_tries = reverts.storage_tries_ref().len(),
        "Computed range trie changesets successfully"
    );

    let reverts = Arc::new(reverts);
    let overlay = if end_block == db_tip_block { Arc::clone(&reverts) } else { Arc::new(overlay) };
    Ok(ComputedTrieChangesets { reverts, overlay })
}

/// Complete trie reverts for a block range and for the range plus its tail.
#[derive(Debug, Clone, Default)]
pub struct ComputedTrieChangesets {
    /// Target values for paths affected by the range, including transient nodes.
    pub reverts: Arc<TrieUpdatesSorted>,
    /// Target values for paths affected from the range start through the database tip.
    pub overlay: Arc<TrieUpdatesSorted>,
}
