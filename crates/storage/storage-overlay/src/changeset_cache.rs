//! Trie changeset caching utilities.
//!
//! This module provides functionality to compute trie changesets for a given block,
//! which represent the old trie node values before the block was processed.
//!
//! It also provides an efficient in-memory cache for these changesets, which is essential for:
//! - **Reorg support**: Quickly access changesets to revert blocks during chain reorganizations
//! - **Memory efficiency**: Explicit eviction releases persisted changesets

use crate::{database_state_frontiers, OverlayManager, OverlayStateProvider};
use alloy_primitives::{map::B256Map, BlockNumber, B256};
use parking_lot::RwLock;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use reth_primitives_traits::{AlloyBlockHeader, FastInstant as Instant, NodePrimitives};
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, PruneCheckpointReader, StageCheckpointReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::trie_cursor::{InMemoryTrieCursorFactory, TrieCursor, TrieCursorFactory};
use reth_trie_common::updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted};
use reth_trie_db::{DatabaseTrieCursorFactory, TrieTableAdapter};
use std::{
    collections::{BTreeMap, HashMap},
    ops::RangeInclusive,
    sync::Arc,
};
use tracing::{debug, warn};

/// Returns recorded block trie updates, or reconstructs them for a persisted block.
///
/// Reconstruction uses the block's changeset paths and looks up their after-values in the
/// masked disk trie, completed with changesets for subsequent blocks through Finish.
///
/// # Errors
///
/// Returns an error if the block is unavailable or historical state cannot be reconstructed.
pub(crate) fn compute_block_trie_updates<N, Provider>(
    overlay_manager: &OverlayManager<N>,
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
{
    reth_trie_db::with_adapter!(provider, |A| {
        compute_block_trie_updates_inner::<_, _, A>(overlay_manager, provider, block_number)
    })
}

fn compute_block_trie_updates_inner<N, Provider, A>(
    overlay_manager: &OverlayManager<N>,
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
    let cache = overlay_manager.changeset_cache();
    let (partial_state_trie, finish) = database_state_frontiers(provider)?;

    if block_number > finish.number {
        return Err(ProviderError::InsufficientChangesets {
            requested: block_number,
            available: 0..=finish.number,
        })
    }
    if let Some(block) = overlay_manager
        .parent_chain(finish.hash)
        .find(|block| block.recovered_block().number() == block_number)
    {
        return Ok((*block.trie_data().sorted.trie_updates).clone())
    }
    if block_number > partial_state_trie.number {
        return Err(ProviderError::StateForNumberNotFound(block_number))
    }

    let changesets =
        cache.get_or_compute_range(overlay_manager, provider, block_number..=block_number)?;
    let reverts =
        cache.get_or_compute_range(overlay_manager, provider, block_number + 1..=finish.number)?;

    // Include deferred blocks: they may have masked older writes in the disk trie.
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

/// Thread-safe changeset cache.
///
/// This type wraps a shared, mutable reference to the cache inner.
/// The `RwLock` enables concurrent reads while ensuring exclusive access for writes.
#[derive(Debug, Clone)]
pub(crate) struct ChangesetCache {
    inner: Arc<RwLock<ChangesetCacheInner>>,
}

impl Default for ChangesetCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangesetCache {
    /// Creates a new cache.
    ///
    /// The cache has no capacity limit and relies on explicit eviction
    /// via the `evict()` method to manage memory usage.
    pub(crate) fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(ChangesetCacheInner::new())) }
    }

    /// Evicts changesets for blocks below the given block number.
    ///
    /// This should be called after blocks are persisted to the database to free
    /// memory for changesets that are no longer needed in the cache.
    ///
    /// # Arguments
    ///
    /// * `up_to_block` - Evict blocks with number < this value. Blocks with number >= this value
    ///   are retained.
    pub(crate) fn evict(&self, up_to_block: BlockNumber) {
        self.inner.write().evict(up_to_block)
    }

    /// Gets or computes trie reverts for a range of blocks.
    ///
    /// Returns complete before-values for paths affected by these blocks, with the oldest value
    /// taking precedence. The result depends only on the canonical range, not database frontiers.
    ///
    /// # Arguments
    ///
    /// * `provider` - Database provider for DB access
    /// * `range` - Block range to accumulate reverts for (inclusive)
    ///
    /// # Returns
    ///
    /// Trie changesets for the requested blocks only. Empty ranges return empty changesets.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Any block in the range is beyond the database tip
    /// - Database access fails
    /// - The in-memory overlay needed to complete the durable trie is unavailable
    /// - Changeset computation fails
    pub(crate) fn get_or_compute_range<N, P>(
        &self,
        overlay_manager: &OverlayManager<N>,
        provider: &P,
        range: RangeInclusive<BlockNumber>,
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
        if range.is_empty() {
            return Ok(Arc::default())
        }
        let start_block = *range.start();
        let end_block = *range.end();
        let timer = Instant::now();
        let end_hash = provider
            .block_hash(end_block)?
            .ok_or(ProviderError::HeaderNotFound(end_block.into()))?;
        let range_key = ChangesetRangeKey::new(start_block, end_block, end_hash);

        if let Some(accumulated_reverts) = self.inner.read().get(&range_key) {
            let elapsed = timer.elapsed();

            debug!(
                target: "trie::changeset_cache",
                ?elapsed,
                start_block,
                end_block,
                num_blocks = end_block.saturating_sub(start_block).saturating_add(1),
                "Changeset cache HIT for block range"
            );

            return Ok(accumulated_reverts)
        }

        warn!(
            target: "trie::changeset_cache",
            start_block,
            end_block,
            "Changeset cache MISS in range, falling back to aggregate DB-based computation"
        );

        let (partial_state_trie, finish) = database_state_frontiers(provider)?;
        if end_block > finish.number {
            return Err(ProviderError::InsufficientChangesets {
                requested: end_block,
                available: 0..=finish.number,
            })
        }
        let overlay = overlay_manager
            .overlay_builder(finish.hash)
            .with_no_reverts()
            .build_state_trie_overlay_at_frontiers(provider, partial_state_trie, finish, true)?;
        let mut forward_updates = overlay_manager
            .parent_chain(finish.hash)
            .take_while(|block| block.recovered_block().number() >= start_block)
            .map(|block| {
                (
                    block.recovered_block().number(),
                    Arc::clone(&block.trie_data().sorted.trie_updates),
                )
            })
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
            num_blocks = end_block.saturating_sub(start_block).saturating_add(1),
            num_account_nodes,
            num_storage_tries,
            "Finished accumulating trie reverts for block range"
        );

        self.inner.write().insert(range_key, accumulated_reverts.clone());

        Ok(accumulated_reverts)
    }
}

/// Cache key for one contiguous range of canonical trie changesets.
///
/// The end hash identifies the chain. Persistence frontiers and blocks after the range do not
/// affect its changesets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChangesetRangeKey {
    start_block: BlockNumber,
    end_block: BlockNumber,
    end_hash: B256,
}

impl ChangesetRangeKey {
    const fn new(start_block: BlockNumber, end_block: BlockNumber, end_hash: B256) -> Self {
        Self { start_block, end_block, end_hash }
    }
}

/// In-memory cache for trie changesets with explicit eviction policy.
///
/// Holds changesets for blocks or block ranges that have been validated but not yet persisted.
/// Keyed by canonical block range. Eviction is controlled
/// explicitly by the engine API tree handler when persistence completes.
///
/// ## Eviction Policy
///
/// Unlike traditional caches with automatic eviction, this cache requires explicit
/// eviction calls. The engine API tree handler calls `evict(block_number)` after
/// blocks are persisted to the database, ensuring changesets remain available
/// until their corresponding blocks are safely on disk.
///
/// ## Metrics
///
/// The cache maintains several metrics for observability:
/// - `hits`: Number of successful cache lookups
/// - `misses`: Number of failed cache lookups
/// - `evictions`: Number of blocks evicted
/// - `size`: Current number of cached blocks
#[derive(Debug)]
struct ChangesetCacheInner {
    /// Cache entries keyed by inclusive block range and end hash.
    entries: HashMap<ChangesetRangeKey, Arc<TrieUpdatesSorted>>,

    /// Range start block to cache keys mapping for eviction.
    range_starts: BTreeMap<BlockNumber, Vec<ChangesetRangeKey>>,

    /// Metrics for monitoring cache behavior
    metrics: ChangesetCacheMetrics,
}

/// Metrics for the changeset cache.
///
/// These metrics provide visibility into cache performance and help identify
/// potential issues like high miss rates.
#[derive(Metrics, Clone)]
#[metrics(scope = "trie.changeset_cache")]
struct ChangesetCacheMetrics {
    /// Cache hit counter
    hits: Counter,

    /// Cache miss counter
    misses: Counter,

    /// Eviction counter
    evictions: Counter,

    /// Current cache size (number of entries)
    size: Gauge,
}

impl Default for ChangesetCacheInner {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangesetCacheInner {
    /// Creates a new empty changeset cache.
    ///
    /// The cache has no capacity limit and relies on explicit eviction
    /// via the `evict()` method to manage memory usage.
    fn new() -> Self {
        Self { entries: HashMap::new(), range_starts: BTreeMap::new(), metrics: Default::default() }
    }

    fn get(&self, key: &ChangesetRangeKey) -> Option<Arc<TrieUpdatesSorted>> {
        match self.entries.get(key) {
            Some(changesets) => {
                self.metrics.hits.increment(1);
                Some(changesets.clone())
            }
            None => {
                self.metrics.misses.increment(1);
                None
            }
        }
    }

    fn insert(&mut self, key: ChangesetRangeKey, changesets: Arc<TrieUpdatesSorted>) {
        debug!(
            target: "trie::changeset_cache",
            ?key,
            cache_size_before = self.entries.len(),
            "Inserting changeset into cache"
        );

        let is_new_entry = self.entries.insert(key, changesets).is_none();

        if is_new_entry {
            self.range_starts.entry(key.start_block).or_default().push(key);
        }

        // Update size metric
        self.metrics.size.set(self.entries.len() as f64);

        debug!(
            target: "trie::changeset_cache",
            ?key,
            cache_size_after = self.entries.len(),
            "Changeset inserted into cache"
        );
    }

    fn evict(&mut self, up_to_block: BlockNumber) {
        debug!(
            target: "trie::changeset_cache",
            up_to_block,
            cache_size_before = self.entries.len(),
            "Starting cache eviction"
        );

        // Find all block numbers that should be evicted (< up_to_block)
        let range_starts_to_evict: Vec<u64> =
            self.range_starts.range(..up_to_block).map(|(num, _)| *num).collect();

        // Remove entries for each block number below threshold
        let mut evicted_count = 0;

        for start_block in &range_starts_to_evict {
            if let Some(keys) = self.range_starts.remove(start_block) {
                debug!(
                    target: "trie::changeset_cache",
                    start_block,
                    num_ranges = keys.len(),
                    "Evicting ranges from cache"
                );
                for key in keys {
                    if self.entries.remove(&key).is_some() {
                        evicted_count += 1;
                    }
                }
            }
        }

        debug!(
            target: "trie::changeset_cache",
            up_to_block,
            evicted_count,
            cache_size_after = self.entries.len(),
            "Finished cache eviction"
        );

        // Update metrics if we evicted anything
        if evicted_count > 0 {
            self.metrics.evictions.increment(evicted_count as u64);
            self.metrics.size.set(self.entries.len() as f64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateTrieOverlay;
    use alloy_consensus::Header;
    use alloy_primitives::{
        keccak256,
        map::{B256Map, HashMap},
        Address, U256,
    };
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    use reth_db::{
        models::{AccountBeforeTx, BlockNumberAddress},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{
        test_utils::create_test_provider_factory, BlockWriter, StaticFileProviderFactory,
        StaticFileSegment, StaticFileWriter,
    };
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint, StageId};
    use reth_storage_api::{StageCheckpointWriter, TrieWriter};
    use reth_trie::{
        changesets::compute_trie_changesets, BranchNodeCompact, ComputedTrieData,
        HashedPostStateSorted, Nibbles, StateRoot, TrieInputSorted,
    };
    use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseHashedPostState, DatabaseStateRoot};

    // Helper function to create empty TrieUpdatesSorted for testing
    fn create_test_changesets() -> Arc<TrieUpdatesSorted> {
        Arc::new(TrieUpdatesSorted::new(vec![], B256Map::default()))
    }

    fn empty_overlay() -> StateTrieOverlay {
        StateTrieOverlay::new(TrieInputSorted::default())
    }

    fn insert_test_changesets(
        cache: &mut ChangesetCacheInner,
        block_hash: B256,
        block_number: BlockNumber,
        changesets: Arc<TrieUpdatesSorted>,
    ) {
        cache.insert(ChangesetRangeKey::new(block_number, block_number, block_hash), changesets);
    }

    fn get_test_changesets(
        cache: &ChangesetCacheInner,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> Option<Arc<TrieUpdatesSorted>> {
        cache.get(&ChangesetRangeKey::new(block_number, block_number, block_hash))
    }

    fn test_account(balance: u64) -> Account {
        Account { balance: U256::from(balance), ..Default::default() }
    }

    fn test_storage(slot: u64, value: u64) -> StorageEntry {
        StorageEntry { key: B256::from(U256::from(slot)), value: U256::from(value) }
    }

    fn seed_headers(
        factory: &impl StaticFileProviderFactory<
            Primitives: reth_primitives_traits::NodePrimitives<BlockHeader = Header>,
        >,
        end_block: BlockNumber,
    ) {
        let static_file_provider = factory.static_file_provider();
        let mut header_writer =
            static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        for block_number in 0..=end_block {
            let header = Header { number: block_number, ..Default::default() };
            header_writer
                .append_header(&header, &B256::with_last_byte(block_number as u8))
                .unwrap();
        }
        header_writer.commit().unwrap();
    }

    fn legacy_compute_range_trie_changesets<Provider>(
        provider: &Provider,
        range: RangeInclusive<BlockNumber>,
    ) -> TrieUpdatesSorted
    where
        Provider: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        let mut accumulated_reverts = TrieUpdatesSorted::default();
        for block_number in range.rev() {
            let changesets = legacy_compute_block_trie_changesets(provider, block_number);
            accumulated_reverts.extend_ref_and_sort(&changesets);
        }
        accumulated_reverts
    }

    fn legacy_compute_block_trie_changesets<Provider>(
        provider: &Provider,
        block_number: BlockNumber,
    ) -> TrieUpdatesSorted
    where
        Provider: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        reth_trie_db::with_adapter!(provider, |A| {
            legacy_compute_block_trie_changesets_inner::<_, A>(provider, block_number)
        })
    }

    fn legacy_compute_block_trie_changesets_inner<Provider, A>(
        provider: &Provider,
        block_number: BlockNumber,
    ) -> TrieUpdatesSorted
    where
        Provider: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + BlockNumReader
            + StorageSettingsCache,
        A: TrieTableAdapter,
    {
        let individual_state_revert =
            HashedPostStateSorted::from_reverts(provider, block_number..=block_number).unwrap();
        let cumulative_state_revert =
            HashedPostStateSorted::from_reverts(provider, (block_number + 1)..).unwrap();

        let mut cumulative_state_revert_prev = cumulative_state_revert.clone();
        cumulative_state_revert_prev.extend_ref_and_sort(&individual_state_revert);

        type DbStateRoot<'a, TX, A> =
            StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

        let input_prev = TrieInputSorted::new(
            Arc::default(),
            Arc::new(cumulative_state_revert_prev.clone()),
            cumulative_state_revert_prev.construct_prefix_sets(),
        );
        let cumulative_trie_updates_prev =
            DbStateRoot::<_, A>::overlay_root_from_nodes_with_updates(
                provider.tx_ref(),
                input_prev,
            )
            .unwrap()
            .1
            .into_sorted();

        let input = TrieInputSorted::new(
            Arc::new(cumulative_trie_updates_prev.clone()),
            Arc::new(cumulative_state_revert),
            individual_state_revert.construct_prefix_sets(),
        );
        let trie_updates =
            DbStateRoot::<_, A>::overlay_root_from_nodes_with_updates(provider.tx_ref(), input)
                .unwrap()
                .1
                .into_sorted();

        let db_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref());
        let state_provider_factory =
            InMemoryTrieCursorFactory::new(db_cursor_factory, &cumulative_trie_updates_prev);

        compute_trie_changesets(&state_provider_factory, &trie_updates).unwrap()
    }

    fn seed_tip_trie_tables<Provider, A>(provider: &Provider)
    where
        Provider: DBProvider + TrieWriter,
        A: TrieTableAdapter,
    {
        type DbStateRoot<'a, TX, A> =
            StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

        let (_, trie_updates) =
            DbStateRoot::<_, A>::from_tx(provider.tx_ref()).root_with_updates().unwrap();
        provider.write_trie_updates(trie_updates).unwrap();
    }

    #[test]
    fn consumers_restore_trie_values_masked_by_deferred_blocks() {
        let manager = OverlayManager::<reth_ethereum_primitives::EthPrimitives>::default();
        let factory = create_test_provider_factory();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(0..11).collect::<Vec<_>>();
        let provider = factory.provider_rw().unwrap();
        for block in &blocks {
            provider.insert_block(block.recovered_block()).unwrap();
        }
        provider
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(10)
                    .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(5) }),
            )
            .unwrap();
        let path = Nibbles::from_nibbles([1]);
        let updates = |mask| {
            Arc::new(TrieUpdatesSorted::new(
                vec![(path, Some(BranchNodeCompact::new(mask, 0, 0, vec![], None)))],
                B256Map::default(),
            ))
        };
        let block4 = updates(1);
        let block9 = updates(2);
        // Block 9 masks block 4's write, so the persisted trie does not contain the block-5 value.
        let masked = TrieUpdatesSorted::disjointed_merge_batch(&[&block4], &[&block9]);
        assert!(masked.is_empty());
        provider.write_trie_updates_sorted(&masked).unwrap();
        provider.commit().unwrap();
        let provider = factory.provider_rw().unwrap();
        for (start, end, before) in [
            (4, 4, Arc::new(TrieUpdatesSorted::new(vec![(path, None)], B256Map::default()))),
            (5, 10, Arc::clone(&block4)),
            (5, 5, Arc::default()),
        ] {
            manager.changeset_cache().inner.write().insert(
                ChangesetRangeKey::new(start, end, blocks[end as usize].recovered_block().hash()),
                before,
            );
        }
        let (frontier, finish) = database_state_frontiers(&*provider).unwrap();
        let overlay = manager
            .overlay_builder(blocks[4].recovered_block().hash())
            .build_state_trie_overlay_at_frontiers(&*provider, frontier, finish, true)
            .unwrap();
        assert_eq!(overlay.input().nodes, block4);
        assert_eq!(manager.compute_block_trie_updates(&*provider, 4).unwrap(), *block4);
    }

    #[test]
    fn calculated_reverts_preserve_deletions_across_frontiers() {
        let factory = create_test_provider_factory();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(0..4).collect::<Vec<_>>();
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let removed = Nibbles::from_nibbles([1]);
        let replaced = Nibbles::from_nibbles([2]);
        let node = BranchNodeCompact::new(0b0001, 0, 0, vec![], None);
        let storage_updates = |storage_nodes| {
            TrieUpdatesSorted::new(
                vec![],
                B256Map::from_iter([(hashed_address, StorageTrieUpdatesSorted { storage_nodes })]),
            )
        };
        let finish_updates =
            Arc::new(storage_updates(vec![(removed, None), (replaced, Some(node.clone()))]));
        let provider_rw = factory.provider_rw().unwrap();
        for block in &blocks {
            provider_rw.insert_block(block.recovered_block()).unwrap();
        }
        provider_rw
            .write_trie_updates_sorted(&storage_updates(vec![(removed, Some(node.clone()))]))
            .unwrap();
        // Reverting the account's creation deletes every storage node visible at Finish.
        provider_rw
            .tx_ref()
            .put::<tables::AccountChangeSets>(1, AccountBeforeTx { address, info: None })
            .unwrap();
        provider_rw
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(2)
                    .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(1) }),
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let manager = OverlayManager::<reth_ethereum_primitives::EthPrimitives>::default();
        for (block, updates) in [
            (&blocks[1], Arc::new(storage_updates(vec![(removed, Some(node))]))),
            (&blocks[2], Arc::clone(&finish_updates)),
            (&blocks[3], Arc::default()),
        ] {
            manager.insert_block(ExecutedBlock::new(
                Arc::clone(&block.recovered_block),
                Arc::clone(&block.execution_output),
                ComputedTrieData::new(Arc::default(), updates),
            ));
        }
        let before = factory.provider().unwrap();
        let cache = manager.changeset_cache();
        assert_eq!(manager.compute_block_trie_updates(&before, 2).unwrap(), *finish_updates);
        let result = cache.get_or_compute_range(&manager, &before, 1..=2).unwrap();
        assert_eq!(
            result.storage_tries_ref()[&hashed_address].storage_nodes_ref(),
            &[(removed, None), (replaced, None)],
        );

        // An empty request returns no changesets, even when the persisted trie lags Finish.
        let end = 2;
        assert!(manager
            .get_or_compute_cached_changesets_range(&before, end + 1..=end)
            .unwrap()
            .is_empty());

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.write_trie_updates_sorted(&finish_updates).unwrap();
        provider_rw.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(3)).unwrap();
        provider_rw.commit().unwrap();
        let after = factory.provider().unwrap();
        let advanced = cache.get_or_compute_range(&manager, &after, 1..=2).unwrap();
        assert!(Arc::ptr_eq(&advanced, &result));

        // A cold calculation at the newer frontier must also serve the older reader.
        cache.evict(2);
        let advanced = cache.get_or_compute_range(&manager, &after, 1..=2).unwrap();
        assert_eq!(advanced, result);

        // An older reader must still receive the deletion after the frontier advances.
        let cached = manager.get_or_compute_cached_changesets_range(&before, 1..=2).unwrap();
        assert!(Arc::ptr_eq(&cached, &advanced));
    }

    #[test]
    fn calculated_historical_range_excludes_tail_reverts() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let path = Nibbles::from_nibbles([1]);
        let base_nodes = Arc::new(TrieUpdatesSorted::new(
            vec![],
            B256Map::from_iter([(
                hashed_address,
                StorageTrieUpdatesSorted {
                    storage_nodes: vec![(
                        path,
                        Some(BranchNodeCompact::new(0b0001, 0, 0, vec![], None)),
                    )],
                },
            )]),
        ));
        // Rewinding block 3 deletes a node introduced by the input overlay.
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(3, AccountBeforeTx { address, info: None })
            .unwrap();
        let state_trie_provider =
            OverlayStateProvider::<&_, reth_ethereum_primitives::EthPrimitives>::new_with_state_trie(
                &*provider,
                StateTrieOverlay::new(TrieInputSorted::new(
                    Arc::clone(&base_nodes),
                    Arc::default(),
                    Default::default(),
                )),
                provider.cached_storage_settings().is_v2(),
            );

        let end = 2;
        for range in [1..=end, end + 1..=end] {
            let result = reth_trie_db::compute_range_trie_changesets(
                &*provider,
                &state_trie_provider,
                &[(3, Arc::clone(&base_nodes))],
                range,
                3,
            )
            .unwrap();
            assert!(result.is_empty());
        }
    }

    #[test]
    fn persisted_range_retains_transient_nodes_without_executed_blocks() {
        let factory = create_test_provider_factory();
        seed_headers(&factory, 2);
        let provider = factory.provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(2)).unwrap();
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(1, AccountBeforeTx { address, info: None })
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                2,
                AccountBeforeTx { address, info: Some(test_account(1)) },
            )
            .unwrap();
        // The account and its storage exist only at block 1. Enough slots to branch below the first
        // nibble ensure the trie stores non-root nodes.
        for slot in 0..257 {
            provider
                .tx_ref()
                .put::<tables::StorageChangeSets>(
                    BlockNumberAddress((1, address)),
                    test_storage(slot, 0),
                )
                .unwrap();
            provider
                .tx_ref()
                .put::<tables::StorageChangeSets>(
                    BlockNumberAddress((2, address)),
                    test_storage(slot, 1),
                )
                .unwrap();
        }
        let state_trie_provider =
            OverlayStateProvider::<&_, reth_ethereum_primitives::EthPrimitives>::new_with_state_trie(
                &*provider,
                empty_overlay(),
                provider.cached_storage_settings().is_v2(),
            );
        let result = reth_trie_db::compute_range_trie_changesets(
            &*provider,
            &state_trie_provider,
            &[],
            1..=2,
            2,
        )
        .unwrap();
        let nodes = result.storage_tries_ref()[&hashed_address].storage_nodes_ref();
        assert!(!nodes.is_empty());
        assert!(nodes.iter().all(|(_, node)| node.is_none()));

        let first = reth_trie_db::compute_range_trie_changesets(
            &*provider,
            &state_trie_provider,
            &[],
            1..=1,
            2,
        )
        .unwrap();
        let second = reth_trie_db::compute_range_trie_changesets(
            &*provider,
            &state_trie_provider,
            &[],
            2..=2,
            2,
        )
        .unwrap();
        let mut merged = second.clone();
        merged.extend_ref_and_sort(&first);
        assert_eq!(result, merged);
        let forward = [(1, Arc::new(second.clone())), (2, Arc::new(first.clone()))];
        assert_eq!(
            reth_trie_db::compute_range_trie_changesets(
                &*provider,
                &state_trie_provider,
                &forward,
                1..=2,
                2
            )
            .unwrap(),
            result,
        );
        let manager = OverlayManager::<reth_ethereum_primitives::EthPrimitives>::default();
        assert_eq!(manager.compute_block_trie_updates(&*provider, 1).unwrap(), second);
        assert_eq!(manager.compute_block_trie_updates(&*provider, 2).unwrap(), first);

        // Recompute block 1 against its own tip instead of a later tip. Its changeset is identical.
        let at_first = OverlayStateProvider::<&_, reth_ethereum_primitives::EthPrimitives>::new_with_state_trie(
            &*provider,
            StateTrieOverlay::new(TrieInputSorted::new(
                Arc::new(second),
                Arc::new(HashedPostStateSorted::from_reverts(&*provider, 2..=2).unwrap()),
                Default::default(),
            )),
            provider.cached_storage_settings().is_v2(),
        );
        let at_first =
            reth_trie_db::compute_range_trie_changesets(&*provider, &at_first, &[], 1..=1, 1)
                .unwrap();
        assert_eq!(first, at_first);
    }

    #[test]
    fn aggregate_range_reverts_to_pre_range_state() {
        let factory = create_test_provider_factory();
        seed_headers(&factory, 3);

        let provider = factory.provider_rw().unwrap();
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let slot1 = B256::from(U256::from(1));
        let slot2 = B256::from(U256::from(2));
        let account1 = test_account(10);
        let account2 = test_account(20);
        let account3 = test_account(30);

        provider.tx_ref().put::<tables::HashedAccounts>(hashed_address, account3).unwrap();
        provider
            .tx_ref()
            .put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key: keccak256(slot1), value: U256::from(25) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key: keccak256(slot2), value: U256::from(20) },
            )
            .unwrap();

        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(1, AccountBeforeTx { address, info: None })
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(2, AccountBeforeTx { address, info: Some(account1) })
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(3, AccountBeforeTx { address, info: Some(account2) })
            .unwrap();

        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(BlockNumberAddress((1, address)), test_storage(1, 0))
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(BlockNumberAddress((1, address)), test_storage(2, 0))
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((2, address)),
                StorageEntry { key: slot1, value: U256::from(10) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((3, address)),
                StorageEntry { key: slot1, value: U256::from(15) },
            )
            .unwrap();

        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(3)).unwrap();
        reth_trie_db::with_adapter!(provider, |A| seed_tip_trie_tables::<_, A>(&*provider));

        let overlay = empty_overlay();
        let state_trie_provider =
            OverlayStateProvider::<&_, reth_ethereum_primitives::EthPrimitives>::new_with_state_trie(
                &*provider,
                overlay,
                provider.cached_storage_settings().is_v2(),
            );
        let actual = reth_trie_db::compute_range_trie_changesets(
            &*provider,
            &state_trie_provider,
            &[],
            1..=3,
            3,
        )
        .unwrap();
        assert!(actual.storage_tries_ref().get(&hashed_address).is_none());

        let cache = ChangesetCache::new();
        let overlay_manager = OverlayManager::<reth_ethereum_primitives::EthPrimitives>::default();
        let from_cache_api =
            cache.get_or_compute_range(&overlay_manager, &*provider, 1..=3).unwrap();
        assert_eq!(*from_cache_api, actual);
        assert_eq!(cache.inner.read().entries.len(), 1);

        let block_changesets =
            cache.get_or_compute_range(&overlay_manager, &*provider, 2..=2).unwrap();
        assert_eq!(*block_changesets, legacy_compute_block_trie_changesets(&*provider, 2));
        assert_eq!(cache.inner.read().entries.len(), 2);
    }

    #[test]
    fn aggregate_range_matches_legacy_per_block_merge_with_storage_wipe() {
        let factory = create_test_provider_factory();
        seed_headers(&factory, 3);

        let provider = factory.provider_rw().unwrap();
        let address = Address::with_last_byte(1);
        let slot1 = B256::from(U256::from(1));
        let slot2 = B256::from(U256::from(2));
        let account1 = test_account(10);
        let account2 = test_account(20);

        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(1, AccountBeforeTx { address, info: None })
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(2, AccountBeforeTx { address, info: Some(account1) })
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(3, AccountBeforeTx { address, info: Some(account2) })
            .unwrap();

        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(BlockNumberAddress((1, address)), test_storage(1, 0))
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(BlockNumberAddress((1, address)), test_storage(2, 0))
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((2, address)),
                StorageEntry { key: slot1, value: U256::from(10) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((3, address)),
                StorageEntry { key: slot1, value: U256::from(15) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((3, address)),
                StorageEntry { key: slot2, value: U256::from(20) },
            )
            .unwrap();

        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(3)).unwrap();
        reth_trie_db::with_adapter!(provider, |A| seed_tip_trie_tables::<_, A>(&*provider));

        let expected = legacy_compute_range_trie_changesets(&*provider, 2..=3);
        let overlay = empty_overlay();
        let state_trie_provider =
            OverlayStateProvider::<&_, reth_ethereum_primitives::EthPrimitives>::new_with_state_trie(
                &*provider,
                overlay,
                provider.cached_storage_settings().is_v2(),
            );
        let actual = reth_trie_db::compute_range_trie_changesets(
            &*provider,
            &state_trie_provider,
            &[],
            2..=3,
            3,
        )
        .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_insert_and_retrieve_single_entry() {
        let mut cache = ChangesetCacheInner::new();
        let hash = B256::random();
        let changesets = create_test_changesets();

        insert_test_changesets(&mut cache, hash, 100, Arc::clone(&changesets));

        // Should be able to retrieve it
        let retrieved = get_test_changesets(&cache, hash, 100);
        assert!(retrieved.is_some());
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn test_insert_multiple_entries() {
        let mut cache = ChangesetCacheInner::new();

        // Insert 10 blocks
        let mut hashes = Vec::new();
        for i in 0..10 {
            let hash = B256::random();
            insert_test_changesets(&mut cache, hash, 100 + i, create_test_changesets());
            hashes.push((100 + i, hash));
        }

        // Should be able to retrieve all
        assert_eq!(cache.entries.len(), 10);
        for (block_number, hash) in hashes {
            assert!(get_test_changesets(&cache, hash, block_number).is_some());
        }
    }

    #[test]
    fn test_eviction_when_explicitly_called() {
        let mut cache = ChangesetCacheInner::new();

        // Insert 15 blocks (0-14)
        let mut hashes = Vec::new();
        for i in 0..15 {
            let hash = B256::random();
            insert_test_changesets(&mut cache, hash, i, create_test_changesets());
            hashes.push((i, hash));
        }

        // All blocks should be present (no automatic eviction)
        assert_eq!(cache.entries.len(), 15);

        // Explicitly evict blocks < 4
        cache.evict(4);

        // Blocks 0-3 should be evicted
        assert_eq!(cache.entries.len(), 11); // blocks 4-14 = 11 blocks

        // Verify blocks 0-3 are evicted
        for i in 0..4 {
            assert!(
                get_test_changesets(&cache, hashes[i as usize].1, i).is_none(),
                "Block {} should be evicted",
                i
            );
        }

        // Verify blocks 4-14 are still present
        for i in 4..15 {
            assert!(
                get_test_changesets(&cache, hashes[i as usize].1, i).is_some(),
                "Block {} should be present",
                i
            );
        }
    }

    #[test]
    fn test_eviction_with_persistence_watermark() {
        let mut cache = ChangesetCacheInner::new();

        // Insert blocks 100-165
        let mut hashes = HashMap::new();
        for i in 100..=165 {
            let hash = B256::random();
            insert_test_changesets(&mut cache, hash, i, create_test_changesets());
            hashes.insert(i, hash);
        }

        // All blocks should be present (no automatic eviction)
        assert_eq!(cache.entries.len(), 66);

        // Simulate persistence up to block 164, with 64-block retention window
        // Eviction threshold = 164 - 64 = 100
        cache.evict(100);

        // Blocks 100-165 should remain (66 blocks)
        assert_eq!(cache.entries.len(), 66);

        // Simulate persistence up to block 165
        // Eviction threshold = 165 - 64 = 101
        cache.evict(101);

        // Blocks 101-165 should remain (65 blocks)
        assert_eq!(cache.entries.len(), 65);
        assert!(get_test_changesets(&cache, hashes[&100], 100).is_none());
        assert!(get_test_changesets(&cache, hashes[&101], 101).is_some());
    }

    #[test]
    fn test_out_of_order_inserts_with_explicit_eviction() {
        let mut cache = ChangesetCacheInner::new();

        // Insert blocks in random order
        let hash_10 = B256::random();
        insert_test_changesets(&mut cache, hash_10, 10, create_test_changesets());

        let hash_5 = B256::random();
        insert_test_changesets(&mut cache, hash_5, 5, create_test_changesets());

        let hash_15 = B256::random();
        insert_test_changesets(&mut cache, hash_15, 15, create_test_changesets());

        let hash_3 = B256::random();
        insert_test_changesets(&mut cache, hash_3, 3, create_test_changesets());

        // All blocks should be present (no automatic eviction)
        assert_eq!(cache.entries.len(), 4);

        // Explicitly evict blocks < 5
        cache.evict(5);

        assert!(get_test_changesets(&cache, hash_3, 3).is_none(), "Block 3 should be evicted");
        assert!(get_test_changesets(&cache, hash_5, 5).is_some(), "Block 5 should be present");
        assert!(get_test_changesets(&cache, hash_10, 10).is_some(), "Block 10 should be present");
        assert!(get_test_changesets(&cache, hash_15, 15).is_some(), "Block 15 should be present");
    }

    #[test]
    fn test_multiple_blocks_same_number() {
        let mut cache = ChangesetCacheInner::new();

        // Insert multiple blocks with same number (side chains)
        let hash_1a = B256::random();
        let hash_1b = B256::random();
        insert_test_changesets(&mut cache, hash_1a, 100, create_test_changesets());
        insert_test_changesets(&mut cache, hash_1b, 100, create_test_changesets());

        // Both should be retrievable
        assert!(get_test_changesets(&cache, hash_1a, 100).is_some());
        assert!(get_test_changesets(&cache, hash_1b, 100).is_some());
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn test_ranges_with_same_numbers_and_different_end_hashes_are_distinct() {
        let mut cache = ChangesetCacheInner::new();
        let path = Nibbles::from_nibbles_unchecked([0x01]);
        let hash_a = B256::with_last_byte(1);
        let hash_b = B256::with_last_byte(2);
        let key_a = ChangesetRangeKey::new(10, 20, hash_a);
        let key_b = ChangesetRangeKey::new(10, 20, hash_b);
        let changesets_a = Arc::new(TrieUpdatesSorted::new(
            vec![(path, Some(BranchNodeCompact::new(0b0001, 0, 0, vec![], None)))],
            B256Map::default(),
        ));
        let changesets_b = Arc::new(TrieUpdatesSorted::new(
            vec![(path, Some(BranchNodeCompact::new(0b0010, 0, 0, vec![], None)))],
            B256Map::default(),
        ));

        cache.insert(key_a, Arc::clone(&changesets_a));
        cache.insert(key_b, Arc::clone(&changesets_b));

        assert_eq!(cache.entries.len(), 2);
        assert_eq!(
            cache.get(&key_a).unwrap().account_nodes_ref(),
            changesets_a.account_nodes_ref()
        );
        assert_eq!(
            cache.get(&key_b).unwrap().account_nodes_ref(),
            changesets_b.account_nodes_ref()
        );

        cache.evict(11);
        assert!(cache.get(&key_a).is_none());
        assert!(cache.get(&key_b).is_none());
    }

    #[test]
    fn test_eviction_removes_all_side_chains() {
        let mut cache = ChangesetCacheInner::new();

        // Insert multiple blocks at same height (side chains)
        let hash_10a = B256::random();
        let hash_10b = B256::random();
        let hash_10c = B256::random();
        insert_test_changesets(&mut cache, hash_10a, 10, create_test_changesets());
        insert_test_changesets(&mut cache, hash_10b, 10, create_test_changesets());
        insert_test_changesets(&mut cache, hash_10c, 10, create_test_changesets());

        let hash_20 = B256::random();
        insert_test_changesets(&mut cache, hash_20, 20, create_test_changesets());

        assert_eq!(cache.entries.len(), 4);

        // Evict blocks < 15 - should remove all three side chains at height 10
        cache.evict(15);

        assert_eq!(cache.entries.len(), 1);
        assert!(get_test_changesets(&cache, hash_10a, 10).is_none());
        assert!(get_test_changesets(&cache, hash_10b, 10).is_none());
        assert!(get_test_changesets(&cache, hash_10c, 10).is_none());
        assert!(get_test_changesets(&cache, hash_20, 20).is_some());
    }
}
