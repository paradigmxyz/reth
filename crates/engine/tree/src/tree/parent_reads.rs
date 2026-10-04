//! Parent storage witnesses owned by one Engine provider/cache view.
//!
//! Witnesses extend the existing successful-parent-read cache contract: they do not promise that
//! a later physical provider read could not fail. Worker reads still execute exactly once. The
//! canonical adapter only substitutes beneath ordinary revm State reads, preserving its cache,
//! account lifecycle, materialization and error ordering for all operations it still performs.

use super::instrumented_state::{
    InstrumentedStateProvider, StateProviderMetrics, StateProviderStats,
};
use alloy_primitives::{Address, B256, U256};
use reth_errors::{ProviderError, ProviderResult};
use reth_evm::parent_reads::{CaptureParentReads, ParentReadBatch, ValidateParentReads};
use reth_execution_cache::{
    CacheFillMode, CacheStats, CachedStateMetrics, CachedStateProvider, SavedCache,
    TxPoolPrewarmCacheSnapshot,
};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    BlockNumReader, ChangeSetReader, DatabaseProviderFactory, DatabaseProviderROFactory,
    EvmStateProviderBox, HistoryReader, PruneCheckpointReader, StageCheckpointReader,
    StateProvider, StorageChangeSetReader, StorageSettingsCache,
};
use reth_revm::{database::StateProviderDatabase, State};
use reth_storage_overlay::OverlayStateProviderFactory;
use revm::{bytecode::Bytecode, state::AccountInfo, Database};
use std::{
    fmt,
    mem::size_of,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Owns the exact factory, selected execution cache and txpool snapshot used by both roles.
/// No API can attach this generation to an arbitrary provider.
pub(crate) struct ParentReadView {
    source: Box<dyn ParentSource>,
    generation: Arc<ViewGeneration>,
}

impl fmt::Debug for ParentReadView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParentReadView")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl ParentReadView {
    pub(super) fn new<P, N>(
        factory: OverlayStateProviderFactory<P, N>,
        saved_cache: SavedCache,
        txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
        payload_hash: B256,
        parent_hash: B256,
        parent_state_root: B256,
    ) -> Option<Arc<Self>>
    where
        N: NodePrimitives,
        P: DatabaseProviderFactory + 'static,
        P::Provider: BlockNumReader
            + PruneCheckpointReader
            + StageCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + StorageSettingsCache
            + HistoryReader
            + 'static,
    {
        if saved_cache.executed_block_hash() != parent_hash ||
            txpool_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.parent_hash() != parent_hash)
        {
            return None
        }
        Some(Arc::new(Self {
            source: Box::new(Source { factory, saved_cache, txpool_snapshot }),
            generation: Arc::new(ViewGeneration {
                live: AtomicBool::new(true),
                _payload_hash: payload_hash,
                _parent_hash: parent_hash,
                _parent_state_root: parent_state_root,
            }),
        }))
    }

    pub(super) fn open_worker(&self) -> ProviderResult<ParentDatabase> {
        self.source.open(None).map(|provider| self.bind(provider))
    }

    pub(super) fn open_canonical(
        &self,
        options: CanonicalOptions,
    ) -> ProviderResult<ParentDatabase> {
        let diagnostics = options.instrumentation.is_some();
        self.source.open(Some(options)).map(|provider| {
            let mut database = self.bind(provider);
            database.certified_reads = diagnostics.then_some(0);
            database
        })
    }

    fn bind(&self, provider: EvmStateProviderBox) -> ParentDatabase {
        ParentDatabase {
            inner: StateProviderDatabase::new(provider),
            generation: Some(Arc::clone(&self.generation)),
            recording: None,
            offered: None,
            certified_reads: None,
        }
    }

    /// Closing does not replace the worker-completion barrier before cache post-state updates.
    pub(super) fn close(&self) {
        self.generation.live.store(false, Ordering::Release);
    }
}

impl Drop for ParentReadView {
    fn drop(&mut self) {
        self.close();
    }
}

/// Existing canonical provider diagnostics, not a provider supplied by a caller.
#[derive(Default)]
pub(crate) struct CanonicalOptions {
    pub(super) cache_metrics: Option<CachedStateMetrics>,
    pub(super) cache_stats: Option<Arc<CacheStats>>,
    pub(super) instrumentation: Option<(StateProviderMetrics, Arc<StateProviderStats>)>,
}

trait ParentSource: Send + Sync {
    fn open(&self, canonical: Option<CanonicalOptions>) -> ProviderResult<EvmStateProviderBox>;
}

struct Source<P, N: NodePrimitives> {
    factory: OverlayStateProviderFactory<P, N>,
    saved_cache: SavedCache,
    txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
}

impl<P, N> ParentSource for Source<P, N>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + HistoryReader
        + 'static,
{
    fn open(&self, canonical: Option<CanonicalOptions>) -> ProviderResult<EvmStateProviderBox> {
        let provider = self.factory.database_provider_ro()?.into_evm_state_provider();
        let cache = self.saved_cache.cache().clone();
        if let Some(options) = canonical {
            let provider = CachedStateProvider::new_with_mode(
                provider,
                cache,
                CacheFillMode::LookupOnly,
                options.cache_metrics,
                options.cache_stats,
            )
            .with_txpool_snapshot(self.txpool_snapshot.clone());
            Ok(if let Some((metrics, stats)) = options.instrumentation {
                Box::new(InstrumentedStateProvider::with_stats(provider, metrics, stats))
            } else {
                Box::new(provider)
            })
        } else {
            Ok(Box::new(
                CachedStateProvider::new_prewarm(provider, cache)
                    .with_txpool_snapshot(self.txpool_snapshot.clone()),
            ))
        }
    }
}

/// Concrete outer database, with no mutable access to its provider binding.
#[derive(Debug)]
pub(crate) struct ParentDatabase {
    inner: StateProviderDatabase<EvmStateProviderBox>,
    generation: Option<Arc<ViewGeneration>>,
    recording: Option<Vec<StorageRead>>,
    offered: Option<StorageRead>,
    /// Canonical-only diagnostics, with no shared counter on the read path.
    certified_reads: Option<usize>,
}

impl ParentDatabase {
    pub(super) fn unbound(provider: EvmStateProviderBox) -> Self {
        Self {
            inner: StateProviderDatabase::new(provider),
            generation: None,
            recording: None,
            offered: None,
            certified_reads: None,
        }
    }

    pub(super) fn is_bound(&self) -> bool {
        self.generation.as_ref().is_some_and(|generation| generation.live.load(Ordering::Acquire))
    }

    pub(super) const fn certified_reads(&self) -> Option<usize> {
        self.certified_reads
    }

    pub(super) fn capture_hooks() -> CaptureParentReads<Self> {
        CaptureParentReads {
            begin: Self::begin,
            storage: Self::capture_storage,
            finish: Self::finish,
            discard: Self::discard,
        }
    }

    pub(super) fn validation_hooks<'a>() -> ValidateParentReads<&'a mut State<Self>> {
        ValidateParentReads {
            offer: |state, batch, index, address, slot, expected| {
                state.database.offered = None;
                if !state.has_bal() && !state.use_preloaded_bundle {
                    state.database.offer(batch, index, address, slot, expected);
                }
            },
            clear: |state| state.database.offered = None,
        }
    }

    fn begin(&mut self) {
        self.offered = None;
        self.recording = self.is_bound().then(Vec::new);
    }

    fn capture_storage(
        &mut self,
        index: usize,
        address: Address,
        slot: U256,
    ) -> ProviderResult<U256> {
        // Always execute the original provider read, including its cache fill and errors.
        // Never consume an offer or reuse an earlier certificate on a worker.
        let value = match self.inner.storage(address, slot) {
            Ok(value) => value,
            Err(error) => {
                self.discard();
                return Err(error)
            }
        };
        if self.is_bound() &&
            let Some(reads) = self.recording.as_mut()
        {
            if reads.last().is_some_and(|read| read.index >= index) || reads.len() == MAX_READS {
                self.recording = None;
            } else {
                if reads.len() == reads.capacity() {
                    let capacity = reads.capacity().max(8).saturating_mul(2).min(MAX_READS);
                    if reads.try_reserve_exact(capacity - reads.len()).is_err() ||
                        reads.capacity() > MAX_READS
                    {
                        self.recording = None;
                        return Ok(value)
                    }
                }
                reads.push(StorageRead { index, address, slot, value });
            }
        }
        Ok(value)
    }

    fn finish(&mut self) -> Option<ParentReadBatch> {
        let reads = self.recording.take()?;
        if reads.is_empty() || !self.is_bound() {
            return None
        }
        let estimated_bytes =
            reads.capacity().checked_mul(size_of::<StorageRead>())?.checked_add(BATCH_OVERHEAD)?;
        if estimated_bytes > MAX_BATCH_BYTES {
            return None
        }
        Some(ParentReadBatch {
            opaque: Arc::new(StorageBatch {
                generation: Arc::clone(self.generation.as_ref()?),
                reads,
            }),
            estimated_bytes,
        })
    }

    fn discard(&mut self) {
        self.recording = None;
        self.offered = None;
    }

    fn offer(
        &mut self,
        batch: &ParentReadBatch,
        index: usize,
        address: Address,
        slot: U256,
        expected: U256,
    ) {
        self.offered = None;
        if self.is_bound() &&
            let Some(batch) = batch.opaque.as_ref().downcast_ref::<StorageBatch>() &&
            self.generation
                .as_ref()
                .is_some_and(|generation| Arc::ptr_eq(generation, &batch.generation)) &&
            let Ok(position) = batch.reads.binary_search_by_key(&index, |read| read.index)
        {
            let read = batch.reads[position];
            if read.address == address && read.slot == slot && read.value == expected {
                self.offered = Some(read);
            }
        }
    }
}

impl Database for ParentDatabase {
    type Error = ProviderError;

    fn basic(&mut self, address: Address) -> ProviderResult<Option<AccountInfo>> {
        self.inner.basic(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> ProviderResult<Bytecode> {
        self.inner.code_by_hash(code_hash)
    }

    fn storage(&mut self, address: Address, slot: U256) -> ProviderResult<U256> {
        if let Some(read) = self.offered.take() &&
            self.is_bound() &&
            read.address == address &&
            read.slot == slot
        {
            if let Some(count) = &mut self.certified_reads {
                *count += 1;
            }
            return Ok(read.value)
        }
        self.inner.storage(address, slot)
    }

    fn block_hash(&mut self, number: u64) -> ProviderResult<B256> {
        self.inner.block_hash(number)
    }
}

// Certificate batches own only this tiny identity, never a provider, factory or execution cache.
#[derive(Debug)]
struct ViewGeneration {
    live: AtomicBool,
    _payload_hash: B256,
    _parent_hash: B256,
    _parent_state_root: B256,
}

#[derive(Debug)]
struct StorageBatch {
    generation: Arc<ViewGeneration>,
    reads: Vec<StorageRead>,
}

#[derive(Clone, Copy, Debug)]
struct StorageRead {
    index: usize,
    address: Address,
    slot: U256,
    value: U256,
}

const MAX_BATCH_BYTES: usize = 32 * 1024 * 1024;
const BATCH_OVERHEAD: usize = size_of::<StorageBatch>() + size_of::<ViewGeneration>() + 128;
const MAX_READS: usize = (MAX_BATCH_BYTES - BATCH_OVERHEAD) / size_of::<StorageRead>();

#[cfg(test)]
mod tests;
