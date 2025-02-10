//! Bitfinity block validator.
use std::collections::HashSet;

use alloy_primitives::B256;
use evm_canister_client::{CanisterClient, EvmCanisterClient};
use itertools::Itertools;
use reth_chain_state::MemoryOverlayStateProvider;
use reth_evm::execute::{BasicBatchExecutor, BatchExecutor};
use reth_evm_ethereum::{
    execute::{EthExecutionStrategy, EthExecutionStrategyFactory},
    EthEvmConfig,
};
use reth_node_types::NodeTypesWithDB;
use reth_primitives::{Block, BlockWithSenders};
use reth_provider::{
    providers::ProviderNodeTypes, ChainSpecProvider as _, ExecutionOutcome,
    HashedPostStateProvider as _, LatestStateProviderRef, ProviderFactory,
};
use reth_revm::{batch::BlockBatchRecord, database::StateProviderDatabase};

/// Block validator for Bitfinity.
///
/// The validator validates the block by executing it and then
/// confirming it on the EVM.
#[derive(Clone, Debug)]
pub struct BitfinityBlockValidator<C, DB>
where
    C: CanisterClient,
    DB: NodeTypesWithDB + Clone,
{
    evm_client: EvmCanisterClient<C>,
    provider_factory: ProviderFactory<DB>,
}

impl<C, DB> BitfinityBlockValidator<C, DB>
where
    C: CanisterClient,
    DB: NodeTypesWithDB<ChainSpec = reth_chainspec::ChainSpec> + ProviderNodeTypes + Clone,
{
    /// Create a new [`BitfinityBlockValidator`].
    pub const fn new(
        evm_client: EvmCanisterClient<C>,
        provider_factory: ProviderFactory<DB>,
    ) -> Self {
        Self { evm_client, provider_factory }
    }

    /// Validate a block.
    pub async fn validate_blocks(&self, blocks: &[Block]) -> eyre::Result<()> {
        if blocks.is_empty() {
            return Ok(())
        }

        let execution_result = self.execute_blocks(blocks)?;
        let validation_hash = self.calculate_validation_hash(execution_result)?;

        let last_block = blocks.iter().last().expect("no blocks").hash_slow();

        self.send_validation_request(last_block, validation_hash).await?;

        Ok(())
    }

    async fn send_validation_request(
        &self,
        block_hash: B256,
        validation_hash: B256,
    ) -> eyre::Result<()> {
        todo!()
    }

    fn calculate_validation_hash(&self, execution_result: ExecutionOutcome) -> eyre::Result<B256> {
        let provider = self.provider_factory.provider()?;
        let state_provider = LatestStateProviderRef::new(&provider);
        let updated_state = state_provider.hashed_post_state(execution_result.state());

        todo!()
    }

    /// Execute block and return validation arguments.
    fn execute_blocks(&self, blocks: &[Block]) -> eyre::Result<ExecutionOutcome> {
        let executor = self.executor();
        let blocks_with_senders: Vec<_> = blocks.iter().map(Self::convert_block).collect();

        match executor.execute_and_verify_batch(&blocks_with_senders) {
            Ok(output) => Ok(output),
            Err(err) => Err(eyre::eyre!("Failed to execute blocks: {err:?}")),
        }
    }

    /// Convert [`Block`] to [`BlockWithSenders`].
    fn convert_block(block: &Block) -> BlockWithSenders {
        use reth_primitives_traits::SignedTransaction;

        let senders: HashSet<_> =
            block.body.transactions.iter().filter_map(|tx| tx.recover_signer()).collect();
        tracing::debug!("Found {} unique senders in block", senders.len());

        BlockWithSenders { block: block.clone(), senders: senders.into_iter().collect() }
    }

    /// Get the block executor for the latest block.
    fn executor(
        &self,
    ) -> BasicBatchExecutor<
        EthExecutionStrategy<
            StateProviderDatabase<MemoryOverlayStateProvider<reth_primitives::EthPrimitives>>,
            EthEvmConfig,
        >,
    > {
        use reth_evm::execute::BlockExecutionStrategyFactory;

        let historical = self.provider_factory.latest().expect("no latest provider");
        let db = MemoryOverlayStateProvider::new(historical, Vec::new());

        BasicBatchExecutor::new(
            EthExecutionStrategyFactory::ethereum(self.provider_factory.chain_spec())
                .create_strategy(StateProviderDatabase::new(db)),
            BlockBatchRecord::default(),
        )
    }
}
