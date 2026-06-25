//! Trie changeset computation and caching utilities.
//!
//! This module provides functionality to compute trie changesets for a given block,
//! which represent the old trie node values before the block was processed.
//!
//! It also provides an efficient in-memory cache for these changesets, which is essential for:
//! - **Reorg support**: Quickly access changesets to revert blocks during chain reorganizations
//! - **Memory efficiency**: Automatic eviction ensures bounded memory usage

use crate::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseTrieCursorFactory, TrieTableAdapter,
};
use alloy_primitives::{map::B256Map, BlockNumber, B256};
use parking_lot::RwLock;
use reth_primitives_traits::FastInstant as Instant;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, StorageChangeSetReader, StorageSettingsCache,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursor, TrieCursorFactory},
    TrieInputSorted,
};
use reth_trie_common::updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted};
use std::{
    collections::{BTreeMap, HashMap},
    ops::RangeInclusive,
    sync::Arc,
};
use tracing::{debug, warn};

#[cfg(test)]
use reth_trie::changesets::compute_trie_changesets;

#[cfg(feature = "metrics")]
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

/// Computes trie changesets for a block.
///
/// # Algorithm
///
/// For block N:
/// 1. Query cumulative `HashedPostState` revert from the database tip to after N.
/// 2. Calculate cumulative `TrieUpdates` revert from the database tip to after N.
/// 3. Query per-block `HashedPostState` revert for block N.
/// 4. Overlay the post-N trie and calculate trie updates to the pre-N state.
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
/// - Block number exceeds database tip
/// - Database access fails
/// - State root computation fails
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
    crate::with_adapter!(provider, |A| {
        compute_range_trie_changesets_inner::<_, A>(
            provider,
            block_number..=block_number,
            db_tip_block,
        )
    })
}

fn compute_range_trie_changesets<Provider>(
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
    crate::with_adapter!(provider, |A| {
        compute_range_trie_changesets_inner::<_, A>(provider, range, db_tip_block)
    })
}

fn compute_range_trie_changesets_inner<Provider, A>(
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
        target: "trie::changeset_cache",
        start_block,
        end_block,
        db_tip_block,
        "Computing range trie changesets from database state"
    );

    // Step 1: collect the state revert for the requested range.
    let range_state_revert = crate::state::from_reverts_auto(provider, range)?;
    let range_prefix_sets = range_state_revert.construct_prefix_sets();

    type DbStateRoot<'a, TX, A> = reth_trie::StateRoot<
        DatabaseTrieCursorFactory<&'a TX, A>,
        DatabaseHashedCursorFactory<&'a TX>,
    >;

    let (range_nodes, range_state) = if end_block == db_tip_block {
        debug!(
            target: "trie::changeset_cache",
            start_block,
            end_block,
            db_tip_block,
            "Skipping tail trie revert computation for tip-ended range"
        );

        (Arc::default(), Arc::new(range_state_revert))
    } else {
        // Step 2: collect the state revert from the database tip to just after the range.
        let tail_state_revert = end_block
            .checked_add(1)
            .map(|next_block| crate::state::from_reverts_auto(provider, next_block..))
            .transpose()?
            .unwrap_or_default();

        // Step 3: compute trie reverts from the database tip to just after the range.
        let tail_input = TrieInputSorted::new(
            Arc::default(),
            Arc::new(tail_state_revert.clone()),
            tail_state_revert.construct_prefix_sets(),
        );
        let tail_trie_revert = DbStateRoot::<_, A>::overlay_root_from_nodes_with_updates(
            provider.tx_ref(),
            tail_input,
        )
        .map_err(ProviderError::other)?
        .1
        .into_sorted();

        // Step 4: overlay the post-range trie and compute the trie revert to the pre-range state.
        let mut pre_range_state_revert = tail_state_revert;
        pre_range_state_revert.extend_ref_and_sort(&range_state_revert);

        (Arc::new(tail_trie_revert), Arc::new(pre_range_state_revert))
    };

    let range_input = TrieInputSorted::new(range_nodes, range_state, range_prefix_sets);
    let range_trie_revert =
        DbStateRoot::<_, A>::overlay_root_from_nodes_with_updates(provider.tx_ref(), range_input)
            .map_err(ProviderError::other)?
            .1
            .into_sorted();

    debug!(
        target: "trie::changeset_cache",
        start_block,
        end_block,
        num_account_nodes = range_trie_revert.account_nodes_ref().len(),
        num_storage_tries = range_trie_revert.storage_tries_ref().len(),
        "Computed range trie changesets successfully"
    );

    Ok(range_trie_revert)
}

/// Computes block trie updates using the changeset cache.
///
/// # Algorithm
///
/// For block N:
/// 1. Get cumulative trie reverts from block N+1 to db tip using the cache
/// 2. Create an overlay cursor factory with these reverts (representing trie state after block N)
/// 3. Walk through account trie changesets for block N
/// 4. For each changed path, look up the current value using the overlay cursor
/// 5. Walk through storage trie changesets for block N
/// 6. For each changed path, look up the current value using the overlay cursor
/// 7. Return the collected trie updates
///
/// # Arguments
///
/// * `cache` - Handle to the changeset cache for retrieving trie reverts
/// * `provider` - Database provider for accessing changesets and block data
/// * `block_number` - Block number to compute trie updates for
///
/// # Returns
///
/// Trie updates representing the state of trie nodes after the block was processed
///
/// # Errors
///
/// Returns error if:
/// - Block number exceeds database tip
/// - Database access fails
/// - Cache retrieval fails
pub fn compute_block_trie_updates<Provider>(
    cache: &ChangesetCache,
    provider: &Provider,
    block_number: BlockNumber,
) -> ProviderResult<TrieUpdatesSorted>
where
    Provider: DBProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    crate::with_adapter!(provider, |A| {
        compute_block_trie_updates_inner::<_, A>(cache, provider, block_number)
    })
}

fn compute_block_trie_updates_inner<Provider, A>(
    cache: &ChangesetCache,
    provider: &Provider,
    block_number: BlockNumber,
) -> ProviderResult<TrieUpdatesSorted>
where
    Provider: DBProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
    A: TrieTableAdapter,
{
    let tx = provider.tx_ref();

    let db_tip_block = provider.best_block_number()?;

    // Step 1: Get the trie changesets for the target block from cache
    let changesets = cache.get_or_compute(provider, block_number)?;

    // Step 2: Get the trie reverts for the state after the target block using the cache
    let reverts = cache.get_or_compute_range(provider, (block_number + 1)..=db_tip_block)?;

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

        storage_tries.insert(
            *hashed_address,
            StorageTrieUpdatesSorted { storage_nodes, is_deleted: storage_changeset.is_deleted },
        );
    }

    Ok(TrieUpdatesSorted::new(account_nodes, storage_tries))
}

/// Thread-safe changeset cache.
///
/// This type wraps a shared, mutable reference to the cache inner.
/// The `RwLock` enables concurrent reads while ensuring exclusive access for writes.
#[derive(Debug, Clone)]
pub struct ChangesetCache {
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
    pub fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(ChangesetCacheInner::new())) }
    }

    /// Retrieves changesets for a block.
    ///
    /// Returns `None` if the block is not in the cache (either evicted or never computed).
    /// Updates hit/miss metrics accordingly.
    pub fn get(
        &self,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> Option<Arc<TrieUpdatesSorted>> {
        self.inner.read().get(&ChangesetRangeKey::single(block_number, block_hash))
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
    pub fn evict(&self, up_to_block: BlockNumber) {
        self.inner.write().evict(up_to_block)
    }

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
    /// Changesets for the block, either from cache or computed on-the-fly.
    pub fn get_or_compute<P>(
        &self,
        provider: &P,
        block_number: BlockNumber,
    ) -> ProviderResult<Arc<TrieUpdatesSorted>>
    where
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        self.get_or_compute_range(provider, block_number..=block_number)
    }

    /// Gets or computes trie reverts for a range of blocks.
    ///
    /// If all blocks in the range are cached, this method retrieves and accumulates those
    /// per-block trie changesets (reverts) in reverse order (newest to oldest), so that older
    /// values take precedence when there are conflicts.
    ///
    /// If any block is missing from cache, this falls back to one aggregate database computation
    /// for the whole range. The aggregate result restores the trie to the state before the range
    /// and is inserted into the range cache.
    ///
    /// # Arguments
    ///
    /// * `provider` - Database provider for DB access and block lookups
    /// * `range` - Block range to accumulate reverts for (inclusive)
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
    pub fn get_or_compute_range<P>(
        &self,
        provider: &P,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Arc<TrieUpdatesSorted>>
    where
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        let db_tip_block = provider.best_block_number()?;

        let start_block = *range.start();
        let end_block = *range.end();

        // If range end is beyond the tip, return an error
        if end_block > db_tip_block {
            return Err(ProviderError::InsufficientChangesets {
                requested: end_block,
                available: 0..=db_tip_block,
            });
        }

        let timer = Instant::now();

        debug!(
            target: "trie::changeset_cache",
            start_block,
            end_block,
            db_tip_block,
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

        let end_block_hash = provider.block_hash(end_block)?.ok_or_else(|| {
            ProviderError::other(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("block hash not found for block number {}", end_block),
            ))
        })?;
        let range_key = ChangesetRangeKey::new(start_block, end_block, end_block_hash);

        if let Some(accumulated_reverts) = self.inner.read().get(&range_key) {
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
                provider.block_hash(block_number)?.ok_or_else(|| {
                    ProviderError::other(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("block hash not found for block number {}", block_number),
                    ))
                })?
            };

            debug!(
                target: "trie::changeset_cache",
                block_number,
                ?block_hash,
                "Looked up block hash for block number in range"
            );

            let block_key = ChangesetRangeKey::single(block_number, block_hash);
            if let Some(changesets) = self.inner.read().get(&block_key) {
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

            self.inner.write().insert(range_key, Arc::clone(&accumulated_reverts));
            return Ok(accumulated_reverts)
        }

        warn!(
            target: "trie::changeset_cache",
            start_block,
            end_block,
            "Changeset cache MISS in range, falling back to aggregate DB-based computation"
        );

        let accumulated_reverts = Arc::new(compute_range_trie_changesets(
            provider,
            start_block..=end_block,
            db_tip_block,
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

        self.inner.write().insert(range_key, Arc::clone(&accumulated_reverts));

        Ok(accumulated_reverts)
    }
}

/// Cache key for one contiguous range of canonical trie changesets.
///
/// The end block hash disambiguates canonical rewrites where the same block numbers later refer to
/// a different chain. For a single block, `start_block == end_block` and `end_block_hash` is that
/// block's hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ChangesetRangeKey {
    start_block: BlockNumber,
    end_block: BlockNumber,
    end_block_hash: B256,
}

impl ChangesetRangeKey {
    const fn new(start_block: BlockNumber, end_block: BlockNumber, end_block_hash: B256) -> Self {
        Self { start_block, end_block, end_block_hash }
    }

    const fn single(block_number: BlockNumber, block_hash: B256) -> Self {
        Self::new(block_number, block_number, block_hash)
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
    /// Cache entries keyed by inclusive block range plus the range's canonical end hash.
    entries: HashMap<ChangesetRangeKey, Arc<TrieUpdatesSorted>>,

    /// Range start block to cache keys mapping for eviction.
    range_starts: BTreeMap<BlockNumber, Vec<ChangesetRangeKey>>,

    /// Metrics for monitoring cache behavior
    #[cfg(feature = "metrics")]
    metrics: ChangesetCacheMetrics,
}

#[cfg(feature = "metrics")]
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
        Self {
            entries: HashMap::new(),
            range_starts: BTreeMap::new(),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
        }
    }

    fn get(&self, key: &ChangesetRangeKey) -> Option<Arc<TrieUpdatesSorted>> {
        match self.entries.get(key) {
            Some(changesets) => {
                #[cfg(feature = "metrics")]
                self.metrics.hits.increment(1);
                Some(Arc::clone(changesets))
            }
            None => {
                #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
        let mut evicted_count = 0;
        #[cfg(not(feature = "metrics"))]
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
        #[cfg(feature = "metrics")]
        if evicted_count > 0 {
            self.metrics.evictions.increment(evicted_count as u64);
            self.metrics.size.set(self.entries.len() as f64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{
        keccak256,
        map::{B256Map, HashMap},
        Address, U256,
    };
    use reth_db::tables;
    use reth_db_api::{
        models::{AccountBeforeTx, BlockNumberAddress},
        transaction::DbTxMut,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{
        test_utils::create_test_provider_factory, StaticFileProviderFactory, StaticFileSegment,
        StaticFileWriter,
    };
    use reth_stages_types::{StageCheckpoint, StageId};
    use reth_storage_api::{StageCheckpointWriter, TrieWriter};
    use reth_trie::{BranchNodeCompact, Nibbles, StateRoot};

    // Helper function to create empty TrieUpdatesSorted for testing
    fn create_test_changesets() -> Arc<TrieUpdatesSorted> {
        Arc::new(TrieUpdatesSorted::new(vec![], B256Map::default()))
    }

    fn insert_test_changesets(
        cache: &mut ChangesetCacheInner,
        block_hash: B256,
        block_number: BlockNumber,
        changesets: Arc<TrieUpdatesSorted>,
    ) {
        cache.insert(ChangesetRangeKey::single(block_number, block_hash), changesets);
    }

    fn get_test_changesets(
        cache: &ChangesetCacheInner,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> Option<Arc<TrieUpdatesSorted>> {
        cache.get(&ChangesetRangeKey::single(block_number, block_hash))
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
        crate::with_adapter!(provider, |A| {
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
            crate::state::from_reverts_auto(provider, block_number..=block_number).unwrap();
        let cumulative_state_revert =
            crate::state::from_reverts_auto(provider, (block_number + 1)..).unwrap();

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
        let overlay_factory =
            InMemoryTrieCursorFactory::new(db_cursor_factory, &cumulative_trie_updates_prev);

        compute_trie_changesets(&overlay_factory, &trie_updates).unwrap()
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
    fn cached_range_merge_keeps_oldest_revert_values() {
        let factory = create_test_provider_factory();
        seed_headers(&factory, 2);

        let provider = factory.provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(2)).unwrap();

        let cache = ChangesetCache::new();
        let path = Nibbles::from_nibbles([0x1, 0x2]);
        let older_node = BranchNodeCompact::new(0b0001, 0, 0, vec![], None);
        let newer_node = BranchNodeCompact::new(0b0010, 0, 0, vec![], None);

        {
            let mut cache = cache.inner.write();
            insert_test_changesets(
                &mut cache,
                B256::with_last_byte(1),
                1,
                Arc::new(TrieUpdatesSorted::new(
                    vec![(path, Some(older_node.clone()))],
                    B256Map::default(),
                )),
            );
            insert_test_changesets(
                &mut cache,
                B256::with_last_byte(2),
                2,
                Arc::new(TrieUpdatesSorted::new(
                    vec![(path, Some(newer_node))],
                    B256Map::default(),
                )),
            );
        }

        let accumulated = cache.get_or_compute_range(&*provider, 1..=2).unwrap();
        assert_eq!(accumulated.account_nodes_ref(), &[(path, Some(older_node))]);
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
        crate::with_adapter!(provider, |A| seed_tip_trie_tables::<_, A>(&*provider));

        let actual = compute_range_trie_changesets(&*provider, 1..=3, 3).unwrap();
        let storage_revert = actual
            .storage_tries_ref()
            .get(&hashed_address)
            .expect("created account storage trie should be deleted by range revert");
        assert!(storage_revert.is_deleted());
        assert!(storage_revert.storage_nodes_ref().is_empty());

        let cache = ChangesetCache::new();
        let from_cache_api = cache.get_or_compute_range(&*provider, 1..=3).unwrap();
        assert_eq!(*from_cache_api, actual);
        assert_eq!(cache.inner.read().entries.len(), 1);

        let block_changesets = cache.get_or_compute(&*provider, 2).unwrap();
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
        crate::with_adapter!(provider, |A| seed_tip_trie_tables::<_, A>(&*provider));

        let expected = legacy_compute_range_trie_changesets(&*provider, 2..=3);
        let actual = compute_range_trie_changesets(&*provider, 2..=3, 3).unwrap();
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
