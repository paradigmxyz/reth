use alloy_consensus::BlockHeader;
use alloy_primitives::BlockNumber;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult, DecodedRevmBal};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedBlock};
use reth_trie::{
    updates::TrieUpdatesSorted, ComputedTrieData, HashedPostStateSorted, LazyTrieData,
};
use std::sync::Arc;

/// Represents an executed block stored in-memory.
#[derive(Clone, Debug)]
pub struct ExecutedBlock<N: NodePrimitives = EthPrimitives> {
    /// Recovered Block
    pub recovered_block: Arc<RecoveredBlock<N::Block>>,
    /// Block's execution outcome.
    pub execution_output: Arc<BlockExecutionOutput<N::Receipt>>,
    /// Deferred trie data produced by execution.
    ///
    /// This allows deferring the computation of the trie data which can be expensive.
    /// The data can be populated asynchronously after the block was validated.
    pub trie_data: LazyTrieData,
    /// The prepared block access list of the block, if one is available.
    ///
    /// `None` means no BAL was available when the block was constructed, not that the block
    /// has none: only blocks the engine validated from a payload that carried a BAL have it
    /// attached. Blocks from before the BAL fork, blocks built or loaded outside engine
    /// validation (payload builder, persistence, tests) and downloaded blocks without a BAL
    /// sidecar leave it unset; the BAL store is the source of truth in that case.
    ///
    /// When present, this carries the raw RLP (for the BAL store) together with the revm
    /// representation (for consumers like the RPC state cache), so that neither has to be
    /// re-derived after validation.
    pub bal: Option<Arc<DecodedRevmBal>>,
}

impl<N: NodePrimitives> Default for ExecutedBlock<N> {
    fn default() -> Self {
        Self {
            recovered_block: Default::default(),
            execution_output: Arc::new(BlockExecutionOutput {
                result: BlockExecutionResult {
                    receipts: Default::default(),
                    requests: Default::default(),
                    gas_used: 0,
                    blob_gas_used: 0,
                },
                state: Default::default(),
            }),
            trie_data: LazyTrieData::ready(ComputedTrieData::default()),
            bal: None,
        }
    }
}

impl<N: NodePrimitives> PartialEq for ExecutedBlock<N> {
    fn eq(&self, other: &Self) -> bool {
        // Trie data is computed asynchronously and the block access list is derived data; neither
        // defines block identity.
        self.recovered_block == other.recovered_block &&
            self.execution_output == other.execution_output
    }
}

impl<N: NodePrimitives> ExecutedBlock<N> {
    /// Create a new [`ExecutedBlock`] with already-computed trie data.
    ///
    /// Use this constructor when trie data is available immediately (e.g., sequencers,
    /// payload builders). This is the safe default path.
    pub fn new(
        recovered_block: Arc<RecoveredBlock<N::Block>>,
        execution_output: Arc<BlockExecutionOutput<N::Receipt>>,
        trie_data: ComputedTrieData,
    ) -> Self {
        Self {
            recovered_block,
            execution_output,
            trie_data: LazyTrieData::ready(trie_data),
            bal: None,
        }
    }

    /// Create a new [`ExecutedBlock`] with deferred trie data.
    ///
    /// This is useful if the trie data is populated somewhere else, e.g. asynchronously
    /// after the block was validated.
    ///
    /// The [`LazyTrieData`] handle allows expensive trie operations (sorting hashed state and
    /// trie updates) to be performed outside the critical validation path by a background task.
    /// This can improve latency for time-sensitive operations like block validation.
    ///
    /// If the data hasn't been populated when [`Self::trie_data()`] is called, the caller waits
    /// for the background task to publish it.
    ///
    /// Use [`Self::new()`] instead when trie data is already computed and available immediately.
    pub const fn with_deferred_trie_data(
        recovered_block: Arc<RecoveredBlock<N::Block>>,
        execution_output: Arc<BlockExecutionOutput<N::Receipt>>,
        trie_data: LazyTrieData,
    ) -> Self {
        Self { recovered_block, execution_output, trie_data, bal: None }
    }

    /// Attaches the prepared block access list of the block, or clears it with `None`.
    pub fn with_bal(mut self, bal: Option<Arc<DecodedRevmBal>>) -> Self {
        self.bal = bal;
        self
    }

    /// Returns the prepared block access list of the block, if one is available.
    ///
    /// `None` only means no BAL was attached to this block; see the `bal` field for details.
    #[inline]
    pub const fn bal(&self) -> Option<&Arc<DecodedRevmBal>> {
        self.bal.as_ref()
    }

    /// Returns a reference to an inner [`SealedBlock`]
    #[inline]
    pub fn sealed_block(&self) -> &SealedBlock<N::Block> {
        self.recovered_block.sealed_block()
    }

    /// Returns a reference to [`RecoveredBlock`]
    #[inline]
    pub fn recovered_block(&self) -> &RecoveredBlock<N::Block> {
        &self.recovered_block
    }

    /// Returns a reference to the block's execution outcome
    #[inline]
    pub fn execution_outcome(&self) -> &BlockExecutionOutput<N::Receipt> {
        &self.execution_output
    }

    /// Returns the trie data, waiting for the background task if not already cached.
    ///
    /// Uses `OnceLock::get_or_init` internally:
    /// - If already computed: returns cached result immediately
    /// - If not computed: first caller waits for the publishing task, others wait for that result
    #[inline]
    #[tracing::instrument(level = "debug", target = "engine::tree", name = "trie_data", skip_all)]
    pub fn trie_data(&self) -> ComputedTrieData {
        self.trie_data.get().clone()
    }

    /// Returns a clone of the deferred trie data handle.
    ///
    /// A handle is a lightweight reference that can be passed to descendants without
    /// forcing trie data to be observed immediately. The actual work runs in the background task.
    #[inline]
    pub fn trie_data_handle(&self) -> LazyTrieData {
        self.trie_data.clone()
    }

    /// Returns the hashed state result of the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn hashed_state(&self) -> Arc<HashedPostStateSorted> {
        self.trie_data().sorted.hashed_state
    }

    /// Returns a reference to the hashed state result of the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn hashed_state_ref(&self) -> &HashedPostStateSorted {
        &self.trie_data.get().sorted.hashed_state
    }

    /// Returns references to the hashed state results of the executed blocks.
    ///
    /// May wait for trie data if any deferred task hasn't completed.
    pub fn hashed_state_refs(blocks: &[Self]) -> Vec<&HashedPostStateSorted> {
        blocks.iter().map(Self::hashed_state_ref).collect()
    }

    /// Returns the trie updates resulting from the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn trie_updates(&self) -> Arc<TrieUpdatesSorted> {
        self.trie_data().sorted.trie_updates
    }

    /// Returns a reference to the trie updates resulting from the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn trie_updates_ref(&self) -> &TrieUpdatesSorted {
        &self.trie_data.get().sorted.trie_updates
    }

    /// Returns references to the trie updates of the executed blocks.
    ///
    /// May wait for trie data if any deferred task hasn't completed.
    pub fn trie_updates_refs(blocks: &[Self]) -> Vec<&TrieUpdatesSorted> {
        blocks.iter().map(Self::trie_updates_ref).collect()
    }

    /// Returns a [`BlockNumber`] of the block.
    #[inline]
    pub fn block_number(&self) -> BlockNumber {
        self.recovered_block.header().number()
    }
}
