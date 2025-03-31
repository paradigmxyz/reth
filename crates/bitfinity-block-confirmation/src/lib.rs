//! Bitfinity block validator.
use alloy_primitives::TxKind;
use did::{BlockConfirmationData, BlockConfirmationResult};
use ethereum_json_rpc_client::{Client, EthJsonRpcClient};

use eyre::{eyre, Ok};
use reth_chain_state::MemoryOverlayStateProvider;
use reth_evm::{
    env::EvmEnv,
    execute::{BasicBlockExecutor, BlockExecutor, BlockExecutorProvider, Executor},
    ConfigureEvm, Evm,
};
use reth_evm_ethereum::{
    execute::EthExecutorProvider, EthEvmConfig
};
use reth_node_types::NodeTypesWithDB;
use reth_primitives::Block;
use reth_primitives_traits::block::RecoveredBlock;
use tracing::{debug, error, info, trace};

use reth_provider::{
    providers::ProviderNodeTypes, ChainSpecProvider as _, ExecutionOutcome, ProviderFactory,
};
use reth_revm::{context::TxEnv, db::{states::bundle_state::BundleRetention, StateBuilder}};

use reth_revm::{
    database::StateProviderDatabase,
    DatabaseCommit,
};
use reth_rpc_eth_types::{cache::db::StateProviderTraitObjWrapper, StateCacheDb};
use reth_trie::{HashedPostState, KeccakKeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;

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
            debug!(target: "bitfinity_block_confirmation::BitfinityBlockConfirmation", "No blocks to confirm");
            return Ok(());
        }

        let execution_result = self.execute_blocks(blocks)?;
        let last_block = blocks.iter().last().expect("no blocks");

        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Calculating confirmation data for block: {}", last_block.number
        );
        let confirmation_data =
            self.calculate_confirmation_data(last_block, execution_result).await?;

        info!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Sending confirmation request for block: {}", last_block.number
        );
        self.send_confirmation_request(confirmation_data).await?;

        Ok(())
    }

    /// Sends the block confirmation request to the EVM.
    async fn send_confirmation_request(
        &self,
        confirmation_data: BlockConfirmationData,
    ) -> eyre::Result<()> {
        let block_number = confirmation_data.block_number;
        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Sending confirmation request for block: {}", block_number
        );
        match self
            .evm_client
            .send_confirm_block(confirmation_data)
            .await
            .map_err(|e| eyre!("{e}"))?
        {
            BlockConfirmationResult::NotConfirmed => {
                error!(
                    target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
                    "Block confirmation request rejected for block: {}", block_number
                );
                Err(eyre!("confirmation request rejected"))
            }
            result => {
                debug!(
                    target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
                    "Block confirmation request accepted with result: {:?}", result
                );
                Ok(())
            }
        }
    }

    /// Calculates confirmation data for a block based on execution result.
    async fn calculate_confirmation_data(
        &self,
        block: &Block,
        execution_result: ExecutionOutcome,
    ) -> eyre::Result<BlockConfirmationData> {
        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Computing PoW hash for block: {}", block.number
        );
        let proof_of_work = self.compute_pow_hash(block, execution_result).await?;
        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "PoW hash computed successfully for block: {}", block.number
        );
        Ok(BlockConfirmationData {
            block_number: block.number,
            hash: block.hash_slow().into(),
            state_root: block.state_root.into(),
            transactions_root: block.transactions_root.into(),
            receipts_root: block.receipts_root.into(),
            proof_of_work,
        })
    }

    /// Execute block and return execution result.
    fn execute_blocks(&self, blocks: &[Block]) -> eyre::Result<ExecutionOutcome> {
        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Executing {} blocks", blocks.len()
        );
        let executor = self.executor();
        let blocks_with_senders: eyre::Result<Vec<_>> = blocks.iter().map(Self::convert_block).collect();
        let blocks_with_senders = blocks_with_senders?;

        let output = executor.execute_batch(&blocks_with_senders)?;
        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Blocks executed successfully"
        );

        Ok(output)
    }

    /// Convert [`Block`] to [`RecoveredBlock`].
    fn convert_block(block: &Block) -> eyre::Result<RecoveredBlock<Block>> {
        use reth_primitives_traits::SignedTransaction;

        let senders: eyre::Result<Vec<_>> = block
            .body
            .transactions
            .iter()
            .enumerate()
            .map(|(index, tx)| {
                tx.recover_signer().map_err(|err| {
                    eyre!("Failed to recover sender for transaction {index} with hash {:?}", tx.hash()).wrap_err(err)
                })
            })
            .collect();
        let senders = senders?;

        Ok(RecoveredBlock::new_unhashed(block.clone(), senders ))
    }

    /// Get the block executor for the latest block.
    fn executor(
        &self,
    ) -> BasicBlockExecutor<EthEvmConfig, StateProviderDatabase<reth_chain_state::MemoryOverlayStateProviderRef<'_>>> {

        let historical = self.provider_factory.latest().expect("no latest provider");
        let db = MemoryOverlayStateProvider::new(historical, Vec::new());

        EthExecutorProvider::ethereum(self.provider_factory.chain_spec())
                .executor(StateProviderDatabase::new(db))
    }

    /// Calculates POW hash
    async fn compute_pow_hash(
        &self,
        block: &Block,
        execution_result: ExecutionOutcome,
    ) -> eyre::Result<did::hash::Hash<alloy_primitives::FixedBytes<32>>> {
        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Computing PoW hash for block: {}, hash: {}",
            block.number, block.hash_slow()
        );
        let exec_state = execution_result.bundle;

        let state_provider = self.provider_factory.latest()?;
        let cache = StateCacheDb::new(StateProviderDatabase::new(StateProviderTraitObjWrapper(
            &state_provider,
        )));

        trace!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Building state with bundle prestate for block: {}", block.number
        );
        let mut state = StateBuilder::new()
            .with_database_ref(&cache)
            .with_bundle_prestate(exec_state)
            .with_bundle_update()
            .build();

        let chain_spec = self.provider_factory.chain_spec();
        let evm_config = EthEvmConfig::new(chain_spec);

        let EvmEnv { cfg_env, block_env } =
            evm_config.evm_env(&block.header);

        let base_fee = block.base_fee_per_gas.map(Into::into);

        trace!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Fetching genesis balances for PoW calculation"
        );
        let genesis_accounts =
            self.evm_client.get_genesis_balances().await.map_err(|e| eyre!("{e}"))?;

        let (from, _) = genesis_accounts.first().ok_or_else(|| eyre!("no genesis accounts"))?;

        let pow_tx =
            did::utils::block_confirmation_pow_transaction(from.clone(), base_fee, None, None);

        // Simple transaction
        let kind = match pow_tx.to {
            Some(to) => TxKind::Call(to.0),
            None => TxKind::Create,
        };
        let tx = TxEnv {
            caller: pow_tx.from.into(),
            gas_limit: pow_tx.gas.0.to(),
            gas_price: pow_tx.gas_price.unwrap_or_default().0.to(),
            kind: kind,
            value: pow_tx.value.0,
            ..Default::default()
        };

        {
            // Setup EVM
            trace!(
                target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
                "Setting up EVM for PoW transaction execution"
            );
            let mut evm = evm_config.evm_with_env(
                &mut state,
                EnvWithHandlerCfg::new_with_cfg_env(cfg_env_with_handler_cfg, block_env, tx),
            );

            debug!(
                target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
                "Executing PoW transaction for block: {}", block.number
            );
            let res = evm.transact(tx)?;

            if !res.result.is_success() {
                error!(
                    target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
                    "PoW transaction failed for block {}: {:?}", block.number, res.result
                );
                eyre::bail!("PoW transaction failed: {:?}", res.result);
            }

            trace!(
                target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
                "Committing PoW transaction state changes"
            );
            evm.db_mut().commit(res.state);
        }

        trace!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Merging state transitions for block: {}", block.number
        );
        state.merge_transitions(BundleRetention::PlainState);
        let bundle = state.take_bundle();

        trace!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Creating hashed post state from bundle state"
        );
        let post_hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state());

        debug!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "Calculating state root for block: {}", block.number
        );
        let state_root =
            StateRoot::overlay_root(self.provider_factory.provider()?.tx_ref(), post_hashed_state)?;

        info!(
            target: "bitfinity_block_confirmation::BitfinityBlockConfirmation",
            "PoW hash computed successfully for block: {}", block.number
        );
        Ok(state_root.into())
    }
}

#[cfg(test)]
mod tests {

    use alloy_consensus::{Block, BlockHeader, Header, SignableTransaction, TxEip2930};
    use alloy_genesis::{Genesis, GenesisAccount};

    use alloy_network::TxSignerSync;
    use alloy_primitives::{hex::FromHex, Address};

    use alloy_signer::Signer;
    use did::{constant::EIP1559_INITIAL_BASE_FEE, U256};

    use jsonrpc_core::{Output, Request, Response, Success, Version};
    use reth_chain_state::test_utils::TestBlockBuilder;
    use reth_chainspec::{
        BaseFeeParams, ChainSpec, ChainSpecBuilder, EthereumHardfork, MAINNET, MIN_TRANSACTION_GAS,
    };
    use reth_db::{test_utils::TempDatabase, DatabaseEnv};
    use reth_db_common::init::init_genesis;
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_evm::execute::{BlockExecutorProvider, Executor};
    use reth_evm_ethereum::execute::EthExecutorProvider;

    use std::{future::Future, sync::Arc};

    use super::*;

    use reth_node_types::{AnyNodeTypesWithEngine, FullNodePrimitives, NodeTypesWithDBAdapter};
    use reth_primitives::{
        BlockBody, BlockExt, EthPrimitives, Receipt, SealedRecoveredBlock, TransactionSigned,
    };
    use reth_provider::{
        test_utils::create_test_provider_factory_with_chain_spec, BlockExecutionOutput,
        BlockReader, BlockWriter, DatabaseProviderFactory, EthStorage, HashedPostStateProvider,
        LatestStateProviderRef, StorageLocation, TransactionVariant,
    };
    use reth_revm::primitives::KECCAK_EMPTY;

    use reth_trie::StateRoot;
    use reth_trie_db::{DatabaseStateRoot, MerklePatriciaTrie};

    #[derive(Clone)]
    struct MockClient {
        pub genesis_accounts: Vec<(did::H160, U256)>,
    }

    impl MockClient {
        fn new(genesis_accounts: Vec<(did::H160, U256)>) -> Self {
            Self { genesis_accounts }
        }
    }

    impl Client for MockClient {
        fn send_rpc_request(
            &self,
            _request: Request,
        ) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<Response>> + Send>> {
            let genesis_accounts = self.genesis_accounts.clone();

            Box::pin(async move {
                let response = Response::Single(Output::Success(Success {
                    jsonrpc: Some(Version::V2),
                    result: serde_json::json!(genesis_accounts),
                    id: jsonrpc_core::Id::Null,
                }));

                anyhow::Ok(response)
            })
        }
    }

    fn make_block<DB>(
        block_number: u64,
        parent_hash: did::H256,
        provider_factory: &ProviderFactory<DB>,
        gas_used: u64,
        transactions: Vec<TransactionSigned>,
    ) -> eyre::Result<RecoveredBlock>
    where
        DB: NodeTypesWithDB<ChainSpec = reth_chainspec::ChainSpec> + ProviderNodeTypes + Clone,
    {
        use reth_primitives_traits::Block as _;
        let parent_block = provider_factory
            .provider()
            .unwrap()
            .block_by_hash(parent_hash.clone().into())
            .unwrap()
            .unwrap();

        let header = parent_block.header();

        let base_fee_per_gas = header.next_block_base_fee(BaseFeeParams::ethereum());
        let state_root =
            StateRoot::from_tx(provider_factory.provider().unwrap().tx_ref()).root().unwrap();
        let chain_spec = provider_factory.chain_spec();
        let block = Block {
            header: Header {
                parent_hash: parent_hash.into(),
                state_root,
                difficulty: chain_spec.fork(EthereumHardfork::Paris).ttd().expect("Paris TTD"),
                number: block_number,
                gas_limit: 8_000_000,
                gas_used,
                base_fee_per_gas,
                ..Default::default()
            },
            body: BlockBody { transactions, ..Default::default() },
        }
        .with_recovered_senders()
        .expect("failed to recover senders");

        Ok(block)
    }

    /// Creates an empty block without transactions
    fn make_empty_block<DB>(
        block_number: u64,
        parent_hash: did::H256,
        provider_factory: &ProviderFactory<DB>,
    ) -> eyre::Result<RecoveredBlock>
    where
        DB: NodeTypesWithDB<ChainSpec = reth_chainspec::ChainSpec> + ProviderNodeTypes + Clone,
    {
        make_block(block_number, parent_hash, provider_factory, 0, vec![])
    }

    /// Creates a block with a single transaction
    fn make_block_with_transaction<DB>(
        block_number: u64,
        parent_hash: did::H256,
        provider_factory: &ProviderFactory<DB>,
        nonce: u64,
    ) -> eyre::Result<RecoveredBlock>
    where
        DB: NodeTypesWithDB<ChainSpec = reth_chainspec::ChainSpec> + ProviderNodeTypes + Clone,
    {
        let chain_spec = provider_factory.chain_spec();
        let mut signer = alloy_signer_local::PrivateKeySigner::from_slice(&[1; 32])?;
        signer.set_chain_id(Some(chain_spec.chain.id()));

        let tx = TxEip2930 {
            chain_id: chain_spec.chain.id(),
            nonce,
            gas_limit: MIN_TRANSACTION_GAS,
            gas_price: EIP1559_INITIAL_BASE_FEE,
            to: TxKind::Call(Address::ZERO),
            value: alloy_primitives::U256::from(1.0 as f64).into(),
            ..Default::default()
        };

        let signed_tx = signer.sign_transaction_sync(&mut tx.clone())?;
        let tx = tx.into_signed(signed_tx);
        let transaction_signed = TransactionSigned::from(tx);

        make_block(block_number, parent_hash, provider_factory, 21_000, vec![transaction_signed])
    }

    /// Converts block execution output to execution outcome for testing
    fn to_execution_outcome(
        block_number: u64,
        block_execution_output: &BlockExecutionOutput<Receipt>,
    ) -> ExecutionOutcome {
        ExecutionOutcome {
            bundle: block_execution_output.state.clone(),
            receipts: block_execution_output.receipts.clone().into(),
            first_block: block_number,
            requests: vec![block_execution_output.requests.clone()],
        }
    }

    /// Executes a block and commits it to the test database
    fn execute_block_and_commit_to_database<N>(
        provider_factory: &ProviderFactory<N>,
        chain_spec: Arc<ChainSpec>,
        block: &RecoveredBlock,
    ) -> eyre::Result<BlockExecutionOutput<Receipt>>
    where
        N: ProviderNodeTypes<
            Primitives: FullNodePrimitives<
                Block = reth_primitives::Block,
                BlockBody = reth_primitives::BlockBody,
                Receipt = reth_primitives::Receipt,
            >,
        >,
    {
        let provider = provider_factory.provider()?;

        // Execute the block to produce a block execution output
        let mut block_execution_output = EthExecutorProvider::ethereum(chain_spec)
            .executor(StateProviderDatabase::new(LatestStateProviderRef::new(&provider)))
            .execute(block)?;
        block_execution_output.state.reverts.sort();

        // Convert the block execution output to an execution outcome for committing to the database
        let execution_outcome = to_execution_outcome(block.number, &block_execution_output);

        let hashed_post_state = provider_factory.hashed_post_state(execution_outcome.state());

        let hashed_state_sorted = hashed_post_state.clone().into_sorted();

        let (_, trie_updates) = StateRoot::overlay_root_with_updates(
            provider_factory.provider()?.tx_ref(),
            hashed_post_state,
        )?;

        // Commit the block's execution outcome to the database
        let provider_rw = provider_factory.provider_rw()?;
        let block = block.clone().seal_slow();
        provider_rw.append_blocks_with_state(
            vec![block],
            execution_outcome,
            hashed_state_sorted,
            trie_updates,
        )?;
        provider_rw.commit()?;

        Ok(block_execution_output)
    }

    fn setup_test_block_validator(
        extended_genesis: Option<(Address, did::U256)>,
    ) -> (
        BitfinityBlockConfirmation<
            MockClient,
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
        SealedRecoveredBlock,
    ) {
        // EVM genesis accounts (similar to the test genesis in EVM)
        let mut original_genesis = vec![
            (
                Address::from_hex("0x756f45e3fa69347a9a973a725e3c98bc4db0b5a0").unwrap(),
                GenesisAccount {
                    balance: U256::from_hex_str("0xad78ebc5ac6200000").unwrap().into(),
                    ..Default::default()
                },
            ),
            (
                Address::from_hex("0xf42f905231c770f0a406f2b768877fb49eee0f21").unwrap(),
                GenesisAccount {
                    balance: U256::from_hex_str("0xaadec983fcff40000").unwrap().into(),
                    ..Default::default()
                },
            ),
        ];

        if let Some((address, balance)) = extended_genesis {
            original_genesis
                .push((address, GenesisAccount { balance: balance.into(), ..Default::default() }));
        }

        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(Genesis {
                    alloc: original_genesis
                        .clone()
                        .iter()
                        .map(|(address, account)| (*address, account.clone()))
                        .collect(),
                    ..MAINNET.genesis.clone()
                })
                .paris_activated()
                .build(),
        );

        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec);

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

        {}

        let mut genesis = vec![];
        for (address, account) in original_genesis {
            genesis.push((address.into(), account.balance.into()));
        }

        let canister_client = EthJsonRpcClient::new(MockClient::new(genesis));

        let block_validator =
            BitfinityBlockConfirmation::new(canister_client, provider_factory.clone());

        (block_validator, genesis_block)
    }

    #[tokio::test]
    async fn bitfinity_test_pow_hash_with_empty_execution() {
        let (block_validator, genesis_block) = setup_test_block_validator(None);

        let block1 = {
            // Create a test block based on genesis.
            let test_block = make_empty_block(
                genesis_block.number + 1,
                genesis_block.hash_slow().into(),
                &block_validator.provider_factory,
            )
            .unwrap();

            // Insert the test block into the database.
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
        let pow =
            block_validator.compute_pow_hash(&block1, ExecutionOutcome::default()).await.unwrap();

        assert_ne!(pow.0, KECCAK_EMPTY, "Proof of work hash should not be empty");

        assert_ne!(original_state, pow.0, "State should change after POW calculation");

        let new_state =
            StateRoot::from_tx(block_validator.provider_factory.provider().unwrap().tx_ref())
                .root()
                .unwrap();

        assert_eq!(original_state, new_state, "State should not change after POW calculation");
    }

    #[tokio::test]
    async fn bitfinity_test_pow_hash_basic_validation() {
        let (block_validator, genesis_block) = setup_test_block_validator(None);

        let mut block_builder = TestBlockBuilder::eth();
        let block = block_builder
            .get_executed_block_with_number(genesis_block.number + 1, genesis_block.hash_slow());

        let outcome = block.execution_outcome();
        let block =
            block.block().clone().seal_with_senders::<reth_primitives::Block>().unwrap().unseal();

        let pow_res = block_validator.compute_pow_hash(&block.block, outcome.clone()).await;

        assert!(pow_res.is_ok());

        assert_ne!(pow_res.unwrap().0, KECCAK_EMPTY, "Proof of work hash should not be empty");
    }

    #[tokio::test]
    async fn bitfinity_test_pow_hash_deterministic() {
        let (block_validator, genesis_block) = setup_test_block_validator(None);
        let mut block_builder = TestBlockBuilder::eth();
        let block = block_builder
            .get_executed_block_with_number(genesis_block.number + 1, genesis_block.hash_slow());

        let outcome = block.execution_outcome();
        let block =
            block.block().clone().seal_with_senders::<reth_primitives::Block>().unwrap().unseal();

        // Compute POW hash twice with the same input
        let pow1 = block_validator.compute_pow_hash(&block.block, outcome.clone()).await.unwrap();
        let pow2 = block_validator.compute_pow_hash(&block.block, outcome.clone()).await.unwrap();

        // Results should be deterministic
        assert_eq!(pow1, pow2, "POW hash computation should be deterministic");
    }

    #[tokio::test]
    async fn bitfinity_test_pow_hash_state_independence() {
        let (block_validator, genesis_block) = setup_test_block_validator(None);
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
            let _ = block_validator.compute_pow_hash(&block.block, outcome.clone()).await.unwrap();

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

    #[tokio::test]
    async fn bitfinity_test_pow_hash_deterministic_with_empty_blocks() {
        // Expected final PoW hash
        let expected_pow_hash = did::H256::from_hex_str(
            "0xe66caa3a5bf8a22c38c33e5fb4cab1bd1b34e9734526a8723c615e9d36ccc45b",
        )
        .unwrap();

        let (block_validator, genesis_block) = setup_test_block_validator(None);

        let block1 = {
            let block = make_empty_block(
                1,
                genesis_block.hash_slow().into(),
                &block_validator.provider_factory,
            )
            .unwrap();
            execute_block_and_commit_to_database(
                &block_validator.provider_factory,
                block_validator.provider_factory.chain_spec().clone(),
                &block,
            )
            .unwrap();

            block
        };

        let block2 = {
            let block =
                make_empty_block(2, block1.hash_slow().into(), &block_validator.provider_factory)
                    .unwrap();

            execute_block_and_commit_to_database(
                &block_validator.provider_factory,
                block_validator.provider_factory.chain_spec().clone(),
                &block,
            )
            .unwrap();

            block
        };

        let block =
            make_empty_block(3, block2.hash_slow().into(), &block_validator.provider_factory)
                .unwrap();

        let block_output = execute_block_and_commit_to_database(
            &block_validator.provider_factory,
            block_validator.provider_factory.chain_spec().clone(),
            &block,
        )
        .unwrap();
        let outcome = to_execution_outcome(block.number, &block_output);

        // Compute POW hash for two different blocks
        let computed_pow =
            block_validator.compute_pow_hash(&block.block, outcome.clone()).await.unwrap();

        assert_eq!(
            expected_pow_hash, computed_pow,
            "POW hash should be deterministic given the same state"
        )
    }

    #[tokio::test]
    async fn bitfinity_test_pow_hash_deterministic_with_transactions() {
        // Expected final PoW hash
        let expected_pow_hash = did::H256::from_hex_str(
            "0xa349b3ca25925decd1c225b8bc3a133c436f3989eb8048e93ee1389c78a1d6a4",
        )
        .unwrap();

        let signer = alloy_signer_local::PrivateKeySigner::from_slice(&[1; 32]).unwrap();

        let (block_validator, genesis_block) =
            setup_test_block_validator(Some((signer.address(), U256::max_value())));

        let block1 = {
            let block = make_block_with_transaction(
                1,
                genesis_block.hash_slow().into(),
                &block_validator.provider_factory,
                0,
            )
            .unwrap();
            execute_block_and_commit_to_database(
                &block_validator.provider_factory,
                block_validator.provider_factory.chain_spec().clone(),
                &block,
            )
            .unwrap();

            block
        };

        let block2 = {
            let block = make_block_with_transaction(
                2,
                block1.hash_slow().into(),
                &block_validator.provider_factory,
                1,
            )
            .unwrap();
            let out = execute_block_and_commit_to_database(
                &block_validator.provider_factory,
                block_validator.provider_factory.chain_spec().clone(),
                &block,
            )
            .unwrap();

            to_execution_outcome(block.number, &out);
            block
        };

        let block = make_block_with_transaction(
            3,
            block2.hash_slow().into(),
            &block_validator.provider_factory,
            2,
        )
        .unwrap();
        let block_output = execute_block_and_commit_to_database(
            &block_validator.provider_factory,
            block_validator.provider_factory.chain_spec().clone(),
            &block,
        )
        .unwrap();

        let outcome = to_execution_outcome(block.number, &block_output);

        let inital_state =
            StateRoot::from_tx(block_validator.provider_factory.provider().unwrap().tx_ref())
                .root()
                .unwrap();

        let computed_pow =
            block_validator.compute_pow_hash(&block.block, outcome.clone()).await.unwrap();

        assert_eq!(
            expected_pow_hash, computed_pow,
            "POW hash should be deterministic given the same state"
        );

        let post_execution_state =
            StateRoot::from_tx(block_validator.provider_factory.provider().unwrap().tx_ref())
                .root()
                .unwrap();

        assert_eq!(
            inital_state, post_execution_state,
            "State should remain unchanged after block execution"
        );
    }
}
