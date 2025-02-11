//! Bitfinity block validator.
use alloy_primitives::{Address, B256, U256};
use did::BlockConfirmationData;
use evm_canister_client::{CanisterClient, EvmCanisterClient};
use eyre::Ok;
use reth_chain_state::MemoryOverlayStateProvider;
use reth_evm::env::EvmEnv;
use reth_evm::execute::{BasicBatchExecutor, BatchExecutor};
use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
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
use reth_provider::{BlockNumReader, DatabaseProviderFactory, HeaderProvider, StateRootProvider};
use reth_revm::db::CacheDB;
use reth_revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg, Env, EnvWithHandlerCfg, TxEnv};
use reth_revm::{batch::BlockBatchRecord, database::StateProviderDatabase};
use reth_revm::{CacheState, DatabaseCommit, Evm, StateBuilder};
use reth_rpc_eth_types::cache::db::StateProviderTraitObjWrapper;
use reth_rpc_eth_types::StateCacheDb;
use reth_trie::{HashedPostState, KeccakKeyHasher};
use reth_trie::{KeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use std::collections::HashSet;
use std::sync::Arc;

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
            return Ok(());
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
        match self.evm_client.confirm_block(confirmation_data).await? {
            Ok(_) => Ok(()),
            Err(err) => Err(eyre::eyre!("{err}")),
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
            proof_of_work: self.calculate_pow_hash(&block),
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
        block: &Block,
    ) -> eyre::Result<did::hash::Hash<alloy_primitives::FixedBytes<32>>> {
        let historical = self.provider_factory.latest().expect("no latest provider");

        let db = StateProviderTraitObjWrapper(&historical);

        let cache = StateCacheDb::new(StateProviderDatabase::new(db));

        let mut state = StateBuilder::new().with_database(cache).with_bundle_update().build();

        let chain_spec = self.provider_factory.chain_spec();
        let evm_config = EthEvmConfig::new(chain_spec);

        let EvmEnv { mut cfg_env_with_handler_cfg, block_env } =
            evm_config.cfg_and_block_env(&block.header);

        cfg_env_with_handler_cfg.cfg_env.disable_balance_check = true;

        let tx = TxEnv {
            caller: Address::from_slice(&[1]),
            gas_limit: 21000,
            gas_price: U256::from(1),
            transact_to: alloy_primitives::TxKind::Call(Address::from_slice(&[2; 20])),
            value: U256::from(1),
            nonce: Some(0),
            ..Default::default()
        };

        {
            // Setup EVM
            let mut evm = evm_config.evm_with_env(
                &mut state,
                EnvWithHandlerCfg::new_with_cfg_env(cfg_env_with_handler_cfg, block_env, tx),
            );

            let res = evm.transact()?;
            evm.db_mut().commit(res.state);
            assert!(res.result.is_success());
        }

        let bundle = state.take_bundle();

        let post_hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state());

        let state_root = db.state_root(post_hashed_state)?;

        Ok(state_root.into())
    }
}
