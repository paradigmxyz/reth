//! Engine stage that executes blocks using the engine's EVM executor path.
//!
//! Uses `batch_executor` + `execute_batch` to process an entire batch of blocks
//! through a single executor instance backed by a single DB state provider.
//! Sender recovery is read from the DB (written by SenderRecoveryStage).
//! Hashed state is computed once from the accumulated BundleState.
//! State root is verified via overlay trie computation with prefix sets
//! (same overlay approach used by the engine's serial state root path).
//! Persistence writes use the same methods that `save_blocks` calls internally.

use alloy_consensus::BlockHeader as _;
use reth_db_api::transaction::DbTxMut;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::ProviderNodeTypes, BlockHashReader, BlockNumReader, BlockReader, DBProvider,
    HashingWriter, HistoryWriter, ProviderFactory,
    StageCheckpointWriter, StateWriter, StaticFileProviderFactory, StatsReader,
    StorageSettingsCache, TrieWriter,
};
use reth_provider::OriginalValuesKnown;
use reth_revm::database::StateProviderDatabase;
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_storage_api::StateWriteConfig;
use reth_trie::{HashedPostState, KeccakKeyHasher, updates::TrieUpdates};
use reth_trie_db::DatabaseStateRoot;

use tracing::*;

/// Default batch size for the engine stage (5000 blocks).
pub const ENGINE_STAGE_DEFAULT_BATCH_SIZE: u64 = 5_000;

/// A pipeline stage that executes blocks using the engine's EVM path.
#[derive(Debug)]
pub struct EngineStage<E: ConfigureEvm, N: ProviderNodeTypes> {
    batch_size: u64,
    evm_config: E,
    provider_factory: ProviderFactory<N>,
}

impl<E: ConfigureEvm, N: ProviderNodeTypes> EngineStage<E, N> {
    /// Create new instance of [`EngineStage`].
    pub fn new(evm_config: E, provider_factory: ProviderFactory<N>, batch_size: u64) -> Self {
        Self { batch_size, evm_config, provider_factory }
    }

    /// Create new instance with default batch size.
    pub fn with_defaults(evm_config: E, provider_factory: ProviderFactory<N>) -> Self {
        Self::new(evm_config, provider_factory, ENGINE_STAGE_DEFAULT_BATCH_SIZE)
    }
}

impl<E, N, Provider> Stage<Provider> for EngineStage<E, N>
where
    E: ConfigureEvm<Primitives = N::Primitives>,
    N: ProviderNodeTypes<Primitives: NodePrimitives<Block: reth_primitives_traits::Block>>,
    Provider: DBProvider<Tx: DbTxMut>
        + BlockReader<Block = <N::Primitives as NodePrimitives>::Block>
        + BlockNumReader
        + BlockHashReader
        + StaticFileProviderFactory<
            Primitives: NodePrimitives<BlockHeader: reth_db_api::table::Value>,
        > + StatsReader
        + StateWriter<Receipt = <N::Primitives as NodePrimitives>::Receipt>
        + StorageSettingsCache
        + HashingWriter
        + TrieWriter
        + StageCheckpointWriter
        + HistoryWriter,
{
    fn id(&self) -> StageId {
        StageId::Execution
    }

    fn execute(
        &mut self,
        provider: &Provider,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let current = input.checkpoint().block_number;
        let target = input.target();

        if current >= target {
            return Ok(ExecOutput { checkpoint: input.checkpoint(), done: true });
        }

        let start = current + 1;
        let batch_end = std::cmp::min(start + self.batch_size - 1, target);

        // Read blocks with senders from DB (senders written by SenderRecoveryStage)
        let recovered = provider
            .block_with_senders_range(start..=batch_end)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        let expected_state_root = recovered
            .last()
            .expect("non-empty block range")
            .header()
            .state_root();

        // EVM execution: single executor, single DB state provider.
        let state_provider = self
            .provider_factory
            .latest()
            .map_err(|e| StageError::Fatal(Box::new(e)))?;
        let db = StateProviderDatabase::new(state_provider);
        let executor = self.evm_config.batch_executor(db);
        let execution_outcome = executor
            .execute_batch(&recovered)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        // Compute hashed state from the accumulated BundleState
        let hashed_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(
            execution_outcome.bundle.state(),
        );
        let sorted_hashed_state = hashed_state.into_sorted();

        // Write state + receipts + changesets
        provider.write_state(
            &execution_outcome,
            OriginalValuesKnown::Yes,
            StateWriteConfig::default(),
        ).map_err(|e| StageError::Fatal(Box::new(e)))?;

        // Write hashed state
        provider.write_hashed_state(&sorted_hashed_state)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        // Compute state root via overlay — uses prefix sets from hashed state to
        // determine which trie nodes need recomputation, then walks the trie with
        // an overlay cursor that combines DB state with in-memory hashed state.
        // This is the same approach as the engine's serial state root path.
        let (state_root, trie_updates) =
            reth_trie::StateRoot::overlay_root_with_updates(
                provider.tx_ref(),
                &sorted_hashed_state,
            )
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        if state_root != expected_state_root {
            return Err(StageError::Fatal(
                format!(
                    "state root mismatch at block {batch_end}: computed {state_root}, expected {expected_state_root}"
                ).into(),
            ));
        }

        // Write trie updates
        provider.write_trie_updates(trie_updates)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        // Update history indices
        provider.update_history_indices(start..=batch_end)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        // Update all pipeline stage checkpoints
        provider.update_pipeline_stages(batch_end, false)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        let done = batch_end >= target;

        info!(
            target: "sync::stages::engine",
            start, batch_end, %state_root,
            "Executed and verified block range"
        );

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(batch_end), done })
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let unwind_to = input.unwind_to;
        provider
            .remove_state_above(unwind_to)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;
        provider
            .update_pipeline_stages(unwind_to, true)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) })
    }
}
