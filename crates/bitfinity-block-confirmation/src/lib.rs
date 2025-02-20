//! Bitfinity block validator.
use alloy_primitives::TxKind;
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
use reth_provider::StateRootProvider;
use reth_provider::{
    providers::ProviderNodeTypes, ChainSpecProvider as _, ExecutionOutcome, ProviderFactory,
};
use reth_revm::db::states::bundle_state::BundleRetention;

use reth_revm::primitives::{EnvWithHandlerCfg, TxEnv};
use reth_revm::{batch::BlockBatchRecord, database::StateProviderDatabase};
use reth_revm::{DatabaseCommit, StateBuilder};
use reth_rpc_eth_types::cache::db::StateProviderTraitObjWrapper;
use reth_rpc_eth_types::StateCacheDb;
use reth_trie::{HashedPostState, KeccakKeyHasher};

use std::collections::HashSet;

/// Block confirmation for Bitfinity.
///
/// Uses custom Bitfinity logic to prove that the block was executed and sends confirmation request
/// to the EVM canister.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct BitfinityBlockConfirmation<C, DB>
where
    C: Client,
    DB: NodeTypesWithDB + Clone,
{
    evm_client: EthJsonRpcClient<C>,
    provider_factory: ProviderFactory<DB>,
}

impl<C, DB> BitfinityBlockConfirmation<C, DB>
where
    C: Client,
    DB: NodeTypesWithDB<ChainSpec = reth_chainspec::ChainSpec> + ProviderNodeTypes + Clone,
{
    /// Create a new [`BitfinityBlockConfirmation`].
    pub const fn new(
        evm_client: EthJsonRpcClient<C>,
        provider_factory: ProviderFactory<DB>,
    ) -> Self {
        Self { evm_client, provider_factory }
    }

    /// Execute the block and send the confirmation request to the EVM.
    pub async fn confirm_blocks(&self, blocks: &[Block]) -> eyre::Result<()> {
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
        Ok(BlockConfirmationData {
            block_number: block.number,
            hash: block.hash_slow().into(),
            state_root: block.state_root.into(),
            transactions_root: block.transactions_root.into(),
            receipts_root: block.receipts_root.into(),
            proof_of_work: self.compute_pow_hash(&block, execution_result)?,
        })
    }

    /// Execute block and return execution result.
    fn execute_blocks(&self, blocks: &[Block]) -> eyre::Result<ExecutionOutcome> {
        let executor = self.executor();
        let blocks_with_senders: Vec<_> = blocks.iter().map(Self::convert_block).collect();

        let output = executor.execute_and_verify_batch(&blocks_with_senders)?;

        Ok(output)
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

    /// Calculates POW hash
    fn compute_pow_hash(
        &self,
        block: &Block,
        execution_result: ExecutionOutcome,
    ) -> eyre::Result<did::hash::Hash<alloy_primitives::FixedBytes<32>>> {
        let state = execution_result.bundle;
        let state_provider = self.provider_factory.latest().expect("no latest provider");
        let cache = StateCacheDb::new(StateProviderDatabase::new(StateProviderTraitObjWrapper(
            &state_provider,
        )));

        let mut state = StateBuilder::new()
            .with_database_ref(&cache)
            .with_bundle_prestate(state)
            .with_bundle_update()
            .build();

        let chain_spec = self.provider_factory.chain_spec();
        let evm_config = EthEvmConfig::new(chain_spec);

        let EvmEnv { mut cfg_env_with_handler_cfg, block_env } =
            evm_config.cfg_and_block_env(&block.header);

        cfg_env_with_handler_cfg.cfg_env.disable_balance_check = true;
        let base_fee = block.base_fee_per_gas;
        let pow_tx = did::utils::pow_transaction(base_fee.map(Into::into));
        // Simple transaction
        let to = match pow_tx.to {
            Some(to) => TxKind::Call(to.0),
            None => TxKind::Create,
        };
        let tx = TxEnv {
            caller: pow_tx.from.into(),
            gas_limit: pow_tx.gas.0.to(),
            gas_price: pow_tx.gas_price.unwrap_or_default().0,
            transact_to: to,
            value: pow_tx.value.0,
            nonce: Some(pow_tx.nonce.0.to()),
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
        state.merge_transitions(BundleRetention::PlainState);
        let bundle = state.take_bundle();

        let post_hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state());

        let state_root = cache.db.state_root(post_hashed_state)?;

        Ok(state_root.into())
    }
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use candid::Principal;

    use evm_canister_client::{EvmCanisterClient, IcCanisterClient};
    use reth_chain_state::test_utils::TestBlockBuilder;
    use reth_chainspec::ChainSpec;
    use reth_db::test_utils::TempDatabase;
    use reth_db::DatabaseEnv;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_engine_primitives::EthEngineTypes;

    use reth_node_types::{AnyNodeTypesWithEngine, NodeTypesWithDBAdapter};
    use reth_primitives::{EthPrimitives, SealedBlockWithSenders};
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_provider::{
        BlockReader, BlockWriter, DatabaseProviderFactory, EthStorage, StorageLocation,
        TransactionVariant,
    };
    use reth_revm::primitives::KECCAK_EMPTY;
    use reth_testing_utils::generators::{self, random_block, BlockParams};
    use reth_trie::StateRoot;
    use reth_trie_db::{DatabaseStateRoot, MerklePatriciaTrie};

    use super::*;

    // Common test setup function to initialize the test environment.
    fn setup_test_block_validator() -> (
        BitfinityBlockValidator<
            IcCanisterClient,
            NodeTypesWithDBAdapter<
                AnyNodeTypesWithEngine<
                    EthPrimitives,
                    EthEngineTypes,
                    ChainSpec,
                    MerklePatriciaTrie,
                    EthStorage,
                >,
                Arc<TempDatabase<DatabaseEnv>>,
            >,
        >,
        SealedBlockWithSenders,
    ) {
        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory).unwrap();
        let genesis_block = provider_factory
            .sealed_block_with_senders(genesis_hash.into(), TransactionVariant::NoHash)
            .unwrap()
            .ok_or_else(|| eyre::eyre!("genesis block not found"))
            .unwrap();

        // Insert genesis block into the underlying database.
        let provider_rw = provider_factory.database_provider_rw().unwrap();
        provider_rw.insert_block(genesis_block.clone(), StorageLocation::Database).unwrap();
        provider_rw.commit().unwrap();

        let canister_client = EvmCanisterClient::new(IcCanisterClient::new(Principal::anonymous()));
        let block_validator =
            BitfinityBlockValidator::new(canister_client, provider_factory.clone());

        (block_validator, genesis_block)
    }

    #[tokio::test]
    async fn test_execute_and_calculate_pow_with_empty_execution() {
        let mut rng = generators::rng();
        let (block_validator, genesis_block) = setup_test_block_validator();

        let block1 = {
            // Create a test block based on genesis.
            let test_block = random_block(
                &mut rng,
                genesis_block.number + 1,
                BlockParams {
                    parent: Some(genesis_block.hash_slow()),
                    tx_count: Some(10),
                    ..Default::default()
                },
            )
            .seal_with_senders::<reth_primitives::Block>()
            .unwrap()
            .unseal();

            //Insert the test block into the database.
            let provider_rw = block_validator.provider_factory.database_provider_rw().unwrap();
            provider_rw
                .insert_block(test_block.clone().seal_slow(), StorageLocation::Database)
                .unwrap();
            provider_rw.commit().unwrap();

            test_block
        };

        let original_state =
            StateRoot::from_tx(block_validator.provider_factory.provider().unwrap().tx_ref())
                .root()
                .unwrap();

        // Calculate the POW hash.
        let pow = block_validator.compute_pow_hash(&block1, ExecutionOutcome::default()).unwrap();

        assert_ne!(pow.0, KECCAK_EMPTY, "Proof of work hash should not be empty");

        assert_ne!(original_state, pow.0, "State should change after POW calculation");

        let new_state =
            StateRoot::from_tx(block_validator.provider_factory.provider().unwrap().tx_ref())
                .root()
                .unwrap();

        assert_eq!(original_state, new_state, "State should not change after POW calculation");
    }

    #[tokio::test]
    async fn test_pow_hash_with_execution_outcome() {
        let (block_validator, genesis_block) = setup_test_block_validator();

        let mut block_builder = TestBlockBuilder::eth();
        let block = block_builder
            .get_executed_block_with_number(genesis_block.number + 1, genesis_block.hash_slow());

        let outcome = block.execution_outcome();
        let block =
            block.block().clone().seal_with_senders::<reth_primitives::Block>().unwrap().unseal();

        let pow_res = block_validator.compute_pow_hash(&block.block, outcome.clone());

        assert!(pow_res.is_ok());

        assert_ne!(pow_res.unwrap().0, KECCAK_EMPTY, "Proof of work hash should not be empty");
    }

    #[tokio::test]
    async fn test_pow_hash_deterministic() {
        let (block_validator, genesis_block) = setup_test_block_validator();
        let mut block_builder = TestBlockBuilder::eth();
        let block = block_builder
            .get_executed_block_with_number(genesis_block.number + 1, genesis_block.hash_slow());

        let outcome = block.execution_outcome();
        let block =
            block.block().clone().seal_with_senders::<reth_primitives::Block>().unwrap().unseal();

        // Compute POW hash twice with the same input
        let pow1 = block_validator.compute_pow_hash(&block.block, outcome.clone()).unwrap();
        let pow2 = block_validator.compute_pow_hash(&block.block, outcome.clone()).unwrap();

        // Results should be deterministic
        assert_eq!(pow1, pow2, "POW hash computation should be deterministic");
    }

    #[tokio::test]
    async fn test_pow_hash_state_independence() {
        let (block_validator, genesis_block) = setup_test_block_validator();
        let mut block_builder = TestBlockBuilder::eth();
        let block = block_builder
            .get_executed_block_with_number(genesis_block.number + 1, genesis_block.hash_slow());

        let outcome = block.execution_outcome();
        let block =
            block.block().clone().seal_with_senders::<reth_primitives::Block>().unwrap().unseal();

        // Get initial state
        let initial_state =
            StateRoot::from_tx(block_validator.provider_factory.provider().unwrap().tx_ref())
                .root()
                .unwrap();

        // Compute POW multiple times
        for _ in 0..3 {
            let _ = block_validator.compute_pow_hash(&block.block, outcome.clone()).unwrap();

            // Check state after each computation
            let current_state =
                StateRoot::from_tx(block_validator.provider_factory.provider().unwrap().tx_ref())
                    .root()
                    .unwrap();

            assert_eq!(
                initial_state, current_state,
                "State should remain unchanged after POW computation"
            );
        }
    }
}
