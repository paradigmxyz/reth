use crate::PendingFlashBlock;
use alloy_eips::{eip2718::WithEncoded, BlockNumberOrTag};
use alloy_primitives::B256;
use op_alloy_rpc_types_engine::OpFlashblockPayloadBase;
use reth_chain_state::{ComputedTrieData, ExecutedBlock};
use reth_errors::RethError;
use reth_evm::{execute::BlockBuilder, ConfigureEvm};
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{
    header::HeaderMut, AlloyBlockHeader, Block, BlockTy, HeaderTy, NodePrimitives, ReceiptTy,
    Recovered, RecoveredBlock,
};
use reth_revm::{cached::CachedReads, database::StateProviderDatabase, db::State};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::trace;

/// The `FlashBlockBuilder` builds [`PendingBlock`] out of a sequence of transactions.
#[derive(Debug)]
pub(crate) struct FlashBlockBuilder<EvmConfig, Provider> {
    evm_config: EvmConfig,
    provider: Provider,
}

impl<EvmConfig, Provider> FlashBlockBuilder<EvmConfig, Provider> {
    pub(crate) const fn new(evm_config: EvmConfig, provider: Provider) -> Self {
        Self { evm_config, provider }
    }

    pub(crate) const fn provider(&self) -> &Provider {
        &self.provider
    }
}

pub(crate) struct BuildArgs<I> {
    pub(crate) base: OpFlashblockPayloadBase,
    pub(crate) transactions: I,
    pub(crate) cached_state: Option<(B256, CachedReads)>,
    pub(crate) last_flashblock_index: u64,
    pub(crate) last_flashblock_hash: B256,
    pub(crate) compute_state_root: bool,
}

impl<N, EvmConfig, Provider> FlashBlockBuilder<EvmConfig, Provider>
where
    N: NodePrimitives,
    EvmConfig: ConfigureEvm<Primitives = N, NextBlockEnvCtx: From<OpFlashblockPayloadBase> + Unpin>,
    Provider: StateProviderFactory
        + BlockReaderIdExt<
            Header = HeaderTy<N>,
            Block = BlockTy<N>,
            Transaction = N::SignedTx,
            Receipt = ReceiptTy<N>,
        > + Unpin,
{
    /// Returns the [`PendingFlashBlock`] made purely out of transactions and
    /// [`OpFlashblockPayloadBase`] in `args`.
    ///
    /// Returns `None` if the flashblock doesn't attach to the latest header.
    pub(crate) fn execute<I: IntoIterator<Item = WithEncoded<Recovered<N::SignedTx>>>>(
        &self,
        mut args: BuildArgs<I>,
    ) -> eyre::Result<Option<(PendingFlashBlock<N>, CachedReads)>> {
        trace!(target: "flashblocks", "Attempting new pending block from flashblocks");

        let latest = self
            .provider
            .latest_header()?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;
        let latest_hash = latest.hash();

        if args.base.parent_hash != latest_hash {
            trace!(target: "flashblocks", flashblock_parent = ?args.base.parent_hash, local_latest=?latest.num_hash(),"Skipping non consecutive flashblock");
            // doesn't attach to the latest block
            return Ok(None)
        }

        let state_provider = self.provider.history_by_block_hash(latest.hash())?;

        let mut request_cache = args
            .cached_state
            .take()
            .filter(|(hash, _)| hash == &latest_hash)
            .map(|(_, state)| state)
            .unwrap_or_default();
        let cached_db = request_cache.as_db_mut(StateProviderDatabase::new(&state_provider));
        let mut state = State::builder().with_database(cached_db).with_bundle_update().build();

        let mut builder = self
            .evm_config
            .builder_for_next_block(&mut state, &latest, args.base.into())
            .map_err(RethError::other)?;

        builder.apply_pre_execution_changes()?;

        for tx in args.transactions {
            let _gas_used = builder.execute_transaction(tx)?;
        }

        // Build the block without computing state root
        trace!(target: "flashblocks", "Building block without state root computation");
        let (execution, bundle_state) = builder.finish_no_state_root(&state_provider)?;
        let hashed_state = state_provider.hashed_post_state(&bundle_state);

        let execution_outcome = ExecutionOutcome::new(
            bundle_state,
            vec![execution.execution_result.receipts],
            execution.block.number(),
            vec![execution.execution_result.requests],
        );

        let pending_block = PendingBlock::with_executed_block(
            Instant::now() + Duration::from_secs(1),
            ExecutedBlock::new(
                execution.block.into(),
                Arc::new(execution_outcome),
                ComputedTrieData::without_trie_input(
                    Arc::new(hashed_state.clone().into_sorted()),
                    Arc::default(),
                ),
            ),
        );
        let pending_flashblock = PendingFlashBlock::new(
            pending_block,
            args.last_flashblock_index,
            args.last_flashblock_hash,
            hashed_state,
            args.compute_state_root,
        );

        Ok(Some((pending_flashblock, request_cache)))
    }

    pub(crate) fn compute_state_root(
        &self,
        computed_block: PendingFlashBlock<N>,
    ) -> eyre::Result<Option<ExecutedBlock<N>>>
    where
        N::BlockHeader: HeaderMut,
    {
        let latest = self
            .provider
            .latest_header()?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;
        let latest_hash = latest.hash();

        if computed_block.parent_hash() != latest_hash {
            trace!(target: "flashblocks", flashblock_parent = ?computed_block.parent_hash(), local_latest=?latest.num_hash(),"Skipping non consecutive flashblock");
            // doesn't attach to the latest block
            return Ok(None)
        }

        let state_provider = self.provider.history_by_block_hash(latest.hash())?;
        let (state_root, trie_updates) =
            state_provider.state_root_with_updates(computed_block.hashed_state.clone())?;

        // Reconstruct the block with the newly computed state root
        let original_block = computed_block.pending.block();
        let senders = original_block.senders().to_vec();
        let (mut header, body) = original_block.clone_block().split();
        header.set_state_root(state_root);

        // Re-populate the executed block
        Ok(Some(ExecutedBlock::new(
            Arc::new(RecoveredBlock::new_unhashed(N::Block::new(header, body), senders)),
            computed_block.pending.executed_block.execution_output.clone(),
            ComputedTrieData::without_trie_input(
                Arc::new(computed_block.hashed_state.into_sorted()),
                Arc::new(trie_updates.into_sorted()),
            ),
        )))
    }
}

impl<EvmConfig: Clone, Provider: Clone> Clone for FlashBlockBuilder<EvmConfig, Provider> {
    fn clone(&self) -> Self {
        Self { evm_config: self.evm_config.clone(), provider: self.provider.clone() }
    }
}
