//! Retrieves canonical trie changesets and reconstructs block trie updates.

use crate::{
    builder::{OverlayBuildMode, OverlaySnapshot, TrieRevertCache},
    changeset_cache::{ChangesetCache, ChangesetRangeKey},
    database_state_frontiers, OverlayBuilder, OverlayContext, OverlayStateProvider,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::B256Map, BlockNumber};
use reth_db_api::transaction::DbTxMut;
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderROFactory, PruneCheckpointReader,
    StageCheckpointReader, StorageChangeSetReader, StorageSettingsCache,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::trie_cursor::{InMemoryTrieCursorFactory, TrieCursor, TrieCursorFactory};
use reth_trie_common::updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted};
use reth_trie_db::{DatabaseTrieCursorFactory, TrieTableAdapter};
use std::{ops::RangeInclusive, sync::Arc};
use tracing::{debug, warn};

impl<N: NodePrimitives> OverlayContext<N> {
    /// Reconstructs the trie nodes written or removed by `block_number`.
    ///
    /// Opens a read transaction paired with the corresponding in-memory blocks. The factory
    /// must not lock this context's in-memory state while opening the transaction.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::BlockNotExecuted`] above the transaction's Finish checkpoint,
    /// [`ProviderError::HeaderNotFound`] for missing canonical headers, or propagates errors
    /// opening the view, resolving its overlay, or computing changesets.
    pub fn compute_block_trie_updates<P>(
        &self,
        factory: &impl DatabaseProviderROFactory<Provider = P>,
        block_number: BlockNumber,
    ) -> ProviderResult<TrieUpdatesSorted>
    where
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + PruneCheckpointReader
            + StageCheckpointReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        let view = self.snapshot_during(|| factory.database_provider_ro()).transpose()?;
        let (provider, snapshot) = OverlaySnapshot::split(view);
        reth_trie_db::with_adapter!(provider, |A| {
            compute_block_trie_updates_inner::<_, _, A>(&snapshot, &provider, block_number)
        })
    }

    /// Gets trie before-values from `from` through Finish, inclusive. The oldest value wins.
    ///
    /// The caller must already hold the database's single writer and must not yet have changed
    /// state or trie tables in this transaction. Persistence committed before the writer opened
    /// is visible to it, and persistence cannot advance while the writer is held. Capture the
    /// in-memory blocks once the writer is open, never open a writer under the snapshot lock.
    pub fn trie_changesets_for_unwind<P>(
        &self,
        provider: &P,
        from: BlockNumber,
    ) -> ProviderResult<Arc<TrieUpdatesSorted>>
    where
        P: DBProvider<Tx: DbTxMut>
            + ChangeSetReader
            + StorageChangeSetReader
            + StageCheckpointReader
            + PruneCheckpointReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        let (provider, snapshot) = OverlaySnapshot::split(self.snapshot_during(|| provider));
        let (partial_state_trie, finish) = database_state_frontiers(provider)?;
        self.manager().changeset_cache().get_or_compute_range(
            provider,
            from..=finish.number,
            partial_state_trie,
            finish,
            TrieRevertCache::ReadWrite,
            &snapshot,
        )
    }
}

fn compute_block_trie_updates_inner<N, Provider, A>(
    snapshot: &OverlaySnapshot<N>,
    provider: &Provider,
    block_number: BlockNumber,
) -> ProviderResult<TrieUpdatesSorted>
where
    N: NodePrimitives,
    Provider: DBProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + BlockNumReader
        + StorageSettingsCache,
    A: TrieTableAdapter,
{
    let tx = provider.tx_ref();
    let cache = snapshot.context().manager().changeset_cache();
    let (partial_state_trie, finish) = database_state_frontiers(provider)?;

    // Step 1: Get the trie changesets for the target block from cache
    let changesets =
        cache.get_or_compute(provider, block_number, partial_state_trie, finish, snapshot)?;

    // No reverts are needed when the target is the Finish block.
    let reverts = if block_number == finish.number {
        Arc::default()
    } else {
        cache.get_or_compute_range(
            provider,
            (block_number + 1)..=finish.number,
            partial_state_trie,
            finish,
            TrieRevertCache::ReadWrite,
            snapshot,
        )?
    };

    // Step 3: Create an InMemoryTrieCursorFactory with the reverts
    // This gives us the trie state as it was after the target block was processed
    let db_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
    let cursor_factory = InMemoryTrieCursorFactory::new(db_cursor_factory, &reverts);

    // Step 4: Collect all account trie nodes that changed in the target block
    let account_nodes_ref = changesets.account_nodes_ref();
    let mut account_nodes = Vec::with_capacity(account_nodes_ref.len());
    let mut account_cursor = cursor_factory.account_trie_cursor()?;

    // Iterate over the account nodes from the changesets
    for (nibbles, _old_node) in account_nodes_ref {
        // Look up the current value of this trie node using the overlay cursor
        let node_value = account_cursor.seek_exact(*nibbles)?.map(|(_, node)| node);
        account_nodes.push((*nibbles, node_value));
    }

    // Step 5: Collect all storage trie nodes that changed in the target block
    let mut storage_tries = B256Map::default();

    // Iterate over the storage tries from the changesets
    for (hashed_address, storage_changeset) in changesets.storage_tries_ref() {
        let mut storage_cursor = cursor_factory.storage_trie_cursor(*hashed_address)?;
        let storage_nodes_ref = storage_changeset.storage_nodes_ref();
        let mut storage_nodes = Vec::with_capacity(storage_nodes_ref.len());

        // Iterate over the storage nodes for this account
        for (nibbles, _old_node) in storage_nodes_ref {
            // Look up the current value of this storage trie node
            let node_value = storage_cursor.seek_exact(*nibbles)?.map(|(_, node)| node);
            storage_nodes.push((*nibbles, node_value));
        }

        storage_tries.insert(*hashed_address, StorageTrieUpdatesSorted { storage_nodes });
    }

    Ok(TrieUpdatesSorted::new(account_nodes, storage_tries))
}

impl ChangesetCache {
    /// Gets changesets from cache, or computes them on-the-fly if missing.
    ///
    /// This is the primary API for retrieving changesets. It checks the cache first, then falls
    /// back to computing from database state if missing.
    ///
    /// # Arguments
    ///
    /// * `block_number` - Block number (for cache insertion and logging)
    /// * `provider` - Database provider for DB access
    ///
    /// # Returns
    ///
    /// Changesets for the block, either from cache or computed on the fly.
    pub(crate) fn get_or_compute<N, P>(
        &self,
        provider: &P,
        block_number: BlockNumber,
        partial_state_trie: BlockNumHash,
        finish: BlockNumHash,
        snapshot: &OverlaySnapshot<N>,
    ) -> ProviderResult<Arc<TrieUpdatesSorted>>
    where
        N: NodePrimitives,
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + StageCheckpointReader
            + PruneCheckpointReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        self.get_or_compute_range(
            provider,
            block_number..=block_number,
            partial_state_trie,
            finish,
            TrieRevertCache::ReadWrite,
            snapshot,
        )
    }

    /// Gets or computes trie reverts for a range of blocks.
    ///
    /// If all blocks in the range are cached, this method retrieves and accumulates those
    /// per-block trie changesets (reverts) in reverse order (newest to oldest), so that older
    /// values take precedence when there are conflicts.
    ///
    /// If any block is missing from cache, this falls back to one aggregate database computation
    /// for the whole range. The aggregate result restores the trie to the state before the range
    /// and is inserted into the range cache in `TrieRevertCache::ReadWrite` mode.
    ///
    /// # Arguments
    ///
    /// * `provider` - Database provider for DB access and block lookups
    /// * `range` - Block range to accumulate reverts for (inclusive)
    /// * `trie_revert_cache` - Whether to retain the accumulated reverts. Callers serving ranges an
    ///   untrusted peer chooses use `ReadOnly`, since entries stay until their start block is
    ///   evicted.
    ///
    /// # Returns
    ///
    /// Accumulated trie reverts for all blocks in the specified range
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Any block in the range is beyond the database tip
    /// - Database access fails
    /// - Block hash lookup fails
    /// - Changeset computation fails
    pub(crate) fn get_or_compute_range<N, P>(
        &self,
        provider: &P,
        range: RangeInclusive<BlockNumber>,
        partial_state_trie: BlockNumHash,
        finish: BlockNumHash,
        trie_revert_cache: TrieRevertCache,
        snapshot: &OverlaySnapshot<N>,
    ) -> ProviderResult<Arc<TrieUpdatesSorted>>
    where
        N: NodePrimitives,
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + StageCheckpointReader
            + PruneCheckpointReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        let start_block = *range.start();
        let end_block = *range.end();
        let timer = Instant::now();

        if end_block > finish.number {
            return Err(ProviderError::BlockNotExecuted {
                requested: end_block,
                executed: finish.number,
            });
        }

        debug!(
            target: "trie::changeset_cache",
            start_block,
            end_block,
            ?partial_state_trie,
            ?finish,
            "Starting get_or_compute_range"
        );

        if start_block > end_block {
            debug!(
                target: "trie::changeset_cache",
                start_block,
                end_block,
                "Empty changeset range requested"
            );
            return Ok(Arc::new(TrieUpdatesSorted::default()))
        }

        let end_block_hash = provider
            .block_hash(end_block)?
            .ok_or(ProviderError::HeaderNotFound(end_block.into()))?;
        let range_key = ChangesetRangeKey::new(start_block, end_block, end_block_hash);

        if let Some(accumulated_reverts) = self.get(&range_key) {
            let elapsed = timer.elapsed();

            debug!(
                target: "trie::changeset_cache",
                ?elapsed,
                start_block,
                end_block,
                ?end_block_hash,
                num_blocks = end_block.saturating_sub(start_block).saturating_add(1),
                "Changeset cache HIT for block range"
            );

            return Ok(accumulated_reverts)
        }

        let mut cached_reverts =
            Vec::with_capacity(end_block.saturating_sub(start_block).saturating_add(1) as usize);
        let mut all_cached = true;

        for block_number in range.rev() {
            // Get the block hash for this block number
            let block_hash = if block_number == end_block {
                end_block_hash
            } else {
                provider
                    .block_hash(block_number)?
                    .ok_or(ProviderError::HeaderNotFound(block_number.into()))?
            };

            debug!(
                target: "trie::changeset_cache",
                block_number,
                ?block_hash,
                "Looked up block hash for block number in range"
            );

            let block_key = ChangesetRangeKey::single(block_number, block_hash);
            if let Some(changesets) = self.get(&block_key) {
                cached_reverts.push(changesets);
            } else {
                all_cached = false;
                break
            }
        }

        if all_cached {
            // `merge_slice` gives precedence to earlier items, so pass reverts oldest-to-newest.
            cached_reverts.reverse();
            let accumulated_reverts = Arc::new(TrieUpdatesSorted::merge_slice(&cached_reverts));
            let elapsed = timer.elapsed();

            let num_account_nodes = accumulated_reverts.account_nodes_ref().len();
            let num_storage_tries = accumulated_reverts.storage_tries_ref().len();

            debug!(
                target: "trie::changeset_cache",
                ?elapsed,
                start_block,
                end_block,
                num_blocks = end_block.saturating_sub(start_block).saturating_add(1),
                num_account_nodes,
                num_storage_tries,
                "Finished accumulating cached trie reverts for block range"
            );

            if matches!(trie_revert_cache, TrieRevertCache::ReadWrite) {
                self.insert(range_key, Arc::clone(&accumulated_reverts));
            }
            return Ok(accumulated_reverts)
        }

        warn!(
            target: "trie::changeset_cache",
            start_block,
            end_block,
            "Changeset cache MISS in range, falling back to aggregate DB-based computation"
        );

        // This overlay is for the canonical Finish block, independent of the requested range.
        let finish_builder =
            OverlayBuilder::new(finish.hash, snapshot.clone(), OverlayBuildMode::Finish);
        let overlay = finish_builder.build_state_trie_overlay_at_frontiers(
            provider,
            partial_state_trie,
            finish,
            true,
        )?;

        // Forward updates are a best-effort optimization to speed up compute_range_trie_changesets,
        // missing blocks do not affect correctness.
        let mut forward_updates = finish_builder
            .parent_state()
            .into_iter()
            .flat_map(|state| state.chain())
            .take_while(|state| state.number() >= start_block)
            .map(|state| (state.number(), state.block_ref().trie_updates()))
            .collect::<Vec<_>>();
        forward_updates.reverse();

        let state_trie_provider = OverlayStateProvider::<&P, N>::new_with_state_trie(
            provider,
            overlay,
            provider.cached_storage_settings().is_v2(),
        );

        let accumulated_reverts = Arc::new(reth_trie_db::compute_range_trie_changesets(
            provider,
            &state_trie_provider,
            &forward_updates,
            start_block..=end_block,
            finish.number,
        )?);

        let elapsed = timer.elapsed();

        let num_account_nodes = accumulated_reverts.account_nodes_ref().len();
        let num_storage_tries = accumulated_reverts.storage_tries_ref().len();

        debug!(
            target: "trie::changeset_cache",
            ?elapsed,
            start_block,
            end_block,
            ?end_block_hash,
            num_blocks = end_block.saturating_sub(start_block).saturating_add(1),
            num_account_nodes,
            num_storage_tries,
            "Finished accumulating trie reverts for block range"
        );

        if matches!(trie_revert_cache, TrieRevertCache::ReadWrite) {
            self.insert(range_key, Arc::clone(&accumulated_reverts));
        }

        Ok(accumulated_reverts)
    }
}
