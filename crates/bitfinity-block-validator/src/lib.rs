//! Bitfinity block validator.
use std::collections::HashSet;

use did::{BlockConfirmationData, BlockConfirmationResult};
use ethereum_json_rpc_client::{Client, EthJsonRpcClient};
use eyre::eyre;
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
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct BitfinityBlockValidator<C, DB>
where
    C: Client,
    DB: NodeTypesWithDB + Clone,
{
    evm_client: EthJsonRpcClient<C>,
    provider_factory: ProviderFactory<DB>,
}

impl<C, DB> BitfinityBlockValidator<C, DB>
where
    C: Client,
    DB: NodeTypesWithDB<ChainSpec = reth_chainspec::ChainSpec> + ProviderNodeTypes + Clone,
{
    /// Create a new [`BitfinityBlockValidator`].
    pub const fn new(
        evm_client: EthJsonRpcClient<C>,
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
        let last_block = blocks.iter().last().expect("no blocks");

        let confirmation_data = self.calculate_confirmation_data(last_block, execution_result)?;

        self.send_confirmation_request(confirmation_data).await?;

        Ok(())
    }

    /// Sends the block confirmation request to the EVM.
    async fn send_confirmation_request(
        &self,
        confirmation_data: BlockConfirmationData,
    ) -> eyre::Result<()> {
        match self.evm_client.send_confirm_block(confirmation_data).await.map_err(|e| eyre!("{e}"))? {
            BlockConfirmationResult::NotConfirmed => Err(eyre!("confirmation request rejected")),
            _ => Ok(()),
        }
    }

    /// Calculates confirmation data for a block based on execution result.
    fn calculate_confirmation_data(
        &self,
        block: &Block,
        execution_result: ExecutionOutcome,
    ) -> eyre::Result<BlockConfirmationData> {
        let provider = self.provider_factory.provider()?;
        let state_provider = LatestStateProviderRef::new(&provider);
        let updated_state = state_provider.hashed_post_state(execution_result.state());

        Ok(BlockConfirmationData {
            block_number: block.number,
            hash: block.hash_slow().into(),
            state_root: block.state_root.into(),
            transactions_root: block.transactions_root.into(),
            receipts_root: block.receipts_root.into(),
            proof_of_work: self.calculate_pow_hash(updated_state),
        })
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

    /// Calculates POW hash based on the given trie state.
    fn calculate_pow_hash(
        &self,
        _updated_state: reth_trie::HashedPostState,
    ) -> did::hash::Hash<alloy_primitives::FixedBytes<32>> {
        // TODO: calculate hash here
        Default::default()
    }
}
