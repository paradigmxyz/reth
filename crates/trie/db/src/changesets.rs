//! Database-backed trie changeset computation utilities.
//!
//! This module reconstructs trie changesets from database state. The resulting changesets contain
//! the old trie node values needed to revert a block or contiguous range of blocks.

use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, TrieTableAdapter};
use alloy_primitives::BlockNumber;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, StorageChangeSetReader, StorageSettingsCache,
};
use reth_storage_errors::provider::ProviderError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory, StateRoot,
    TrieInputSorted,
};
use reth_trie_common::updates::TrieUpdatesSorted;
use std::{ops::RangeInclusive, sync::Arc};
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
pub fn compute_block_trie_changesets<Provider>(
    provider: &Provider,
    block_number: BlockNumber,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: DBProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    let db_tip_block = provider.best_block_number()?;
    let tip_input = TrieInputSorted::default();
    crate::with_adapter!(provider, |A| {
        compute_range_trie_changesets_inner::<_, A>(
            provider,
            block_number..=block_number,
            db_tip_block,
            &tip_input,
        )
    })
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
pub fn compute_range_trie_changesets<Provider>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
    db_tip_block: BlockNumber,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: DBProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    compute_range_trie_changesets_with_tip(
        provider,
        range,
        db_tip_block,
        &TrieInputSorted::default(),
    )
}

/// Computes aggregate trie changesets for an inclusive block range against a logical tip overlay.
///
/// `tip_input` contains trie and hashed-state updates between the durable trie frontier and
/// `db_tip_block`. The returned changesets therefore restore the logical trie from the state after
/// `range.end()` to the state before `range.start()`, even if trie persistence trails the database
/// tip.
///
/// # Errors
///
/// Returns an error if the range exceeds `db_tip_block`, database access fails, or state root
/// computation fails.
pub fn compute_range_trie_changesets_with_tip<Provider>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
    db_tip_block: BlockNumber,
    tip_input: &TrieInputSorted,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: DBProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    crate::with_adapter!(provider, |A| {
        compute_range_trie_changesets_inner::<_, A>(provider, range, db_tip_block, tip_input)
    })
}

fn compute_range_trie_changesets_inner<Provider, A>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
    db_tip_block: BlockNumber,
    tip_input: &TrieInputSorted,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: DBProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
    A: TrieTableAdapter,
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
    let range_state_revert = crate::state::from_reverts_auto(provider, range)?;
    let range_prefix_sets = range_state_revert.construct_prefix_sets();

    let (range_nodes, range_state) = if end_block == db_tip_block {
        debug!(
            target: "trie::changesets",
            start_block,
            end_block,
            db_tip_block,
            "Skipping tail trie revert computation for tip-ended range"
        );

        (Arc::default(), Arc::new(range_state_revert))
    } else {
        // Collect the state revert from the database tip to just after the range.
        let tail_state_revert = end_block
            .checked_add(1)
            .map(|next_block| crate::state::from_reverts_auto(provider, next_block..))
            .transpose()?
            .unwrap_or_default();

        // Compute trie reverts from the database tip to just after the range.
        let tail_input = TrieInputSorted::new(
            Arc::default(),
            Arc::new(tail_state_revert.clone()),
            tail_state_revert.construct_prefix_sets(),
        );
        let tail_trie_revert =
            compute_revert_trie_updates::<_, A>(provider, tip_input, tail_input)?;

        // Overlay the post-range trie and compute the trie revert to the pre-range state.
        let mut pre_range_state_revert = tail_state_revert;
        pre_range_state_revert.extend_ref_and_sort(&range_state_revert);

        (Arc::new(tail_trie_revert), Arc::new(pre_range_state_revert))
    };

    let range_input = TrieInputSorted::new(range_nodes, range_state, range_prefix_sets);
    let range_trie_revert = compute_revert_trie_updates::<_, A>(provider, tip_input, range_input)?;

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

/// Computes trie updates that restore a pre-range state from the supplied reverts.
fn compute_revert_trie_updates<Provider, A>(
    provider: &Provider,
    tip_input: &TrieInputSorted,
    input: TrieInputSorted,
) -> Result<TrieUpdatesSorted, ProviderError>
where
    Provider: DBProvider,
    A: TrieTableAdapter,
{
    StateRoot::new(
        InMemoryTrieCursorFactory::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref()),
                tip_input.nodes.as_ref(),
            ),
            input.nodes.as_ref(),
        ),
        HashedPostStateCursorFactory::new(
            HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(provider.tx_ref()),
                tip_input.state.as_ref(),
            ),
            input.state.as_ref(),
        ),
    )
    .with_prefix_sets(input.prefix_sets.freeze())
    // Revert prefix sets identify changed branches, but not necessarily every child needed to
    // encode the restored branch after a collapse or expansion.
    .with_walk_all_changed_branch_children(true)
    .root_with_updates()
    .map(|(_, updates)| updates.into_sorted())
    .map_err(ProviderError::other)
}
