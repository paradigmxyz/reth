//! Ethereum block executor.

use crate::{
    dao_fork::{DAO_HARDFORK_BENEFICIARY, DAO_HARDKFORK_ACCOUNTS},
    verify::verify_receipts,
    EthEvmConfig,
};
use reth_evm::{
    execute::{
        BatchBlockExecutionOutput, BatchExecutor, BlockExecutionInput, BlockExecutionOutput,
        BlockExecutorProvider, Executor,
    },
    ConfigureEvm,
};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    provider::ProviderError,
};
use reth_primitives::{
    BlockNumber, BlockWithSenders, ChainSpec, GotExpected, Hardfork, Header, PruneModes, Receipt,
    Receipts, Withdrawals, MAINNET, U256,
};
use reth_revm::{
    batch::{BlockBatchRecord, BlockExecutorStats},
    db::states::bundle_state::BundleRetention,
    state_change::{apply_beacon_root_contract_call, post_block_balance_increments},
    Evm, State,
};
use revm_primitives::{
    db::{Database, DatabaseCommit},
    BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ResultAndState,
};
use std::sync::Arc;
use tracing::debug;

/// Provides executors to execute regular ethereum blocks
#[derive(Debug, Clone)]
pub struct EthExecutorProvider<EvmConfig = EthEvmConfig> {
    chain_spec: Arc<ChainSpec>,
    evm_config: EvmConfig,
}

impl EthExecutorProvider {
    /// Creates a new default ethereum executor provider.
    pub fn ethereum(chain_spec: Arc<ChainSpec>) -> Self {
        Self::new(chain_spec, Default::default())
    }

    /// Returns a new provider for the mainnet.
    pub fn mainnet() -> Self {
        Self::ethereum(MAINNET.clone())
    }
}

impl<EvmConfig> EthExecutorProvider<EvmConfig> {
    /// Creates a new executor provider.
    pub fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, evm_config }
    }
}

impl<EvmConfig> EthExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    fn eth_executor<DB>(&self, db: DB) -> EthBlockExecutor<EvmConfig, DB>
    where
        DB: Database<Error = ProviderError>,
    {
        EthBlockExecutor::new(
            self.chain_spec.clone(),
            self.evm_config.clone(),
            State::builder().with_database(db).with_bundle_update().without_state_clear().build(),
        )
    }
}

impl<EvmConfig> BlockExecutorProvider for EthExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    type Executor<DB: Database<Error = ProviderError>> = EthBlockExecutor<EvmConfig, DB>;

    type BatchExecutor<DB: Database<Error = ProviderError>> = EthBatchExecutor<EvmConfig, DB>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        self.eth_executor(db)
    }

    fn batch_executor<DB>(&self, db: DB, prune_modes: PruneModes) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        let executor = self.eth_executor(db);
        EthBatchExecutor {
            executor,
            batch_record: BlockBatchRecord::new(prune_modes),
            stats: BlockExecutorStats::default(),
        }
    }
}

/// Helper container type for EVM with chain spec.
#[derive(Debug, Clone)]
struct EthEvmExecutor<EvmConfig> {
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
}

impl<EvmConfig> EthEvmExecutor<EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    /// Executes the transactions in the block and returns the receipts.
    ///
    /// This applies the pre-execution changes, and executes the transactions.
    ///
    /// # Note
    ///
    /// It does __not__ apply post-execution changes.
    fn execute_pre_and_transactions<Ext, DB>(
        &self,
        block: &BlockWithSenders,
        mut evm: Evm<'_, Ext, &mut State<DB>>,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError>
    where
        DB: Database<Error = ProviderError>,
    {
        // apply pre execution changes
        apply_beacon_root_contract_call(
            &self.chain_spec,
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut evm,
        )?;

        // execute transactions
        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        for (sender, transaction) in block.transactions_with_sender() {
            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            EvmConfig::fill_tx_env(evm.tx_mut(), transaction, *sender);

            // Execute transaction.
            let ResultAndState { result, state } = evm.transact().map_err(move |err| {
                // Ensure hash is calculated for error log, if not already done
                BlockValidationError::EVM {
                    hash: transaction.recalculate_hash(),
                    error: err.into(),
                }
            })?;
            evm.db_mut().commit(state);

            // append gas used
            cumulative_gas_used += result.gas_used();

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(
                #[allow(clippy::needless_update)] // side-effect of optimism fields
                Receipt {
                    tx_type: transaction.tx_type(),
                    // Success flag was added in `EIP-658: Embedding transaction status code in
                    // receipts`.
                    success: result.is_success(),
                    cumulative_gas_used,
                    // convert to reth log
                    logs: result.into_logs(),
                    ..Default::default()
                },
            );
        }
        drop(evm);

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            let receipts = Receipts::from_block_receipt(receipts);
            return Err(BlockValidationError::BlockGasUsed {
                gas: GotExpected { got: cumulative_gas_used, expected: block.gas_used },
                gas_spent_by_tx: receipts.gas_spent_by_tx()?,
            }
            .into())
        }

        Ok((receipts, cumulative_gas_used))
    }
}

/// A basic Ethereum block executor.
///
/// Expected usage:
/// - Create a new instance of the executor.
/// - Execute the block.
#[derive(Debug)]
pub struct EthBlockExecutor<EvmConfig, DB> {
    /// Chain specific evm config that's used to execute a block.
    executor: EthEvmExecutor<EvmConfig>,
    /// The state to use for execution
    state: State<DB>,
}

impl<EvmConfig, DB> EthBlockExecutor<EvmConfig, DB> {
    /// Creates a new Ethereum block executor.
    pub fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig, state: State<DB>) -> Self {
        Self { executor: EthEvmExecutor { chain_spec, evm_config }, state }
    }

    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        &self.executor.chain_spec
    }

    /// Returns mutable reference to the state that wraps the underlying database.
    #[allow(unused)]
    fn state_mut(&mut self) -> &mut State<DB> {
        &mut self.state
    }
}

impl<EvmConfig, DB> EthBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error = ProviderError>,
{
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_env_for_block(&self, header: &Header, total_difficulty: U256) -> EnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        EvmConfig::fill_cfg_and_block_env(
            &mut cfg,
            &mut block_env,
            self.chain_spec(),
            header,
            total_difficulty,
        );

        EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default())
    }

    /// Execute a single block and apply the state changes to the internal state.
    ///
    /// Returns the receipts of the transactions in the block and the total gas used.
    ///
    /// Returns an error if execution fails or receipt verification fails.
    fn execute_and_verify(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        // 1. prepare state on new block
        self.on_new_block(&block.header);

        // 2. configure the evm and execute
        let env = self.evm_env_for_block(&block.header, total_difficulty);

        let (receipts, gas_used) = {
            let evm = self.executor.evm_config.evm_with_env(&mut self.state, env);
            self.executor.execute_pre_and_transactions(block, evm)
        }?;

        // 3. apply post execution changes
        self.post_execution(block, total_difficulty)?;

        // Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is required for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec().is_byzantium_active_at_block(block.header.number) {
            if let Err(error) = verify_receipts(
                block.header.receipts_root,
                block.header.logs_bloom,
                receipts.iter(),
            ) {
                debug!(target: "evm", %error, ?receipts, "receipts verification failed");
                return Err(error)
            };
        }

        Ok((receipts, gas_used))
    }

    /// Apply settings before a new block is executed.
    pub(crate) fn on_new_block(&mut self, header: &Header) {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag = self.chain_spec().is_spurious_dragon_active_at_block(header.number);
        self.state.set_state_clear_flag(state_clear_flag);
    }

    /// Apply post execution state changes, including block rewards, withdrawals, and irregular DAO
    /// hardfork state change.
    pub fn post_execution(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        let mut balance_increments = post_block_balance_increments(
            self.chain_spec(),
            block.number,
            block.difficulty,
            block.beneficiary,
            block.timestamp,
            total_difficulty,
            &block.ommers,
            block.withdrawals.as_ref().map(Withdrawals::as_ref),
        );

        // Irregular state change at Ethereum DAO hardfork
        if self.chain_spec().fork(Hardfork::Dao).transitions_at_block(block.number) {
            // drain balances from hardcoded addresses.
            let drained_balance: u128 = self
                .state
                .drain_balances(DAO_HARDKFORK_ACCOUNTS)
                .map_err(|_| BlockValidationError::IncrementBalanceFailed)?
                .into_iter()
                .sum();

            // return balance to DAO beneficiary.
            *balance_increments.entry(DAO_HARDFORK_BENEFICIARY).or_default() += drained_balance;
        }
        // increment balances
        self.state
            .increment_balances(balance_increments)
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;

        Ok(())
    }
}

impl<EvmConfig, DB> Executor<DB> for EthBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error = ProviderError>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    /// Executes the block and commits the state changes.
    ///
    /// Returns the receipts of the transactions in the block.
    ///
    /// Returns an error if the block could not be executed or failed verification.
    ///
    /// State changes are committed to the database.
    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;
        let (receipts, gas_used) = self.execute_and_verify(block, total_difficulty)?;

        // NOTE: we need to merge keep the reverts for the bundle retention
        self.state.merge_transitions(BundleRetention::Reverts);

        Ok(BlockExecutionOutput { state: self.state.take_bundle(), receipts, gas_used })
    }
}

/// An executor for a batch of blocks.
///
/// State changes are tracked until the executor is finalized.
#[derive(Debug)]
pub struct EthBatchExecutor<EvmConfig, DB> {
    /// The executor used to execute single blocks
    ///
    /// All state changes are committed to the [State].
    executor: EthBlockExecutor<EvmConfig, DB>,
    /// Keeps track of the batch and records receipts based on the configured prune mode
    batch_record: BlockBatchRecord,
    stats: BlockExecutorStats,
}

impl<EvmConfig, DB> EthBatchExecutor<EvmConfig, DB> {
    /// Returns mutable reference to the state that wraps the underlying database.
    #[allow(unused)]
    fn state_mut(&mut self) -> &mut State<DB> {
        self.executor.state_mut()
    }
}

impl<EvmConfig, DB> BatchExecutor<DB> for EthBatchExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error = ProviderError>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BatchBlockExecutionOutput;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;
        let (receipts, _gas_used) = self.executor.execute_and_verify(block, total_difficulty)?;

        // prepare the state according to the prune mode
        let retention = self.batch_record.bundle_retention(block.number);
        self.executor.state.merge_transitions(retention);

        // store receipts in the set
        self.batch_record.save_receipts(receipts)?;

        if self.batch_record.first_block().is_none() {
            self.batch_record.set_first_block(block.number);
        }

        Ok(())
    }

    fn finalize(mut self) -> Self::Output {
        self.stats.log_debug();

        BatchBlockExecutionOutput::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
        )
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        self.batch_record.set_tip(tip);
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.executor.state.bundle_state.size_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{
        bytes,
        constants::{BEACON_ROOTS_ADDRESS, SYSTEM_ADDRESS},
        keccak256, Account, Block, Bytes, ChainSpecBuilder, ForkCondition, B256,
    };
    use reth_revm::{
        database::StateProviderDatabase, test_utils::StateProviderTest, TransitionState,
    };
    use std::collections::HashMap;

    static BEACON_ROOT_CONTRACT_CODE: Bytes = bytes!("3373fffffffffffffffffffffffffffffffffffffffe14604d57602036146024575f5ffd5b5f35801560495762001fff810690815414603c575f5ffd5b62001fff01545f5260205ff35b5f5ffd5b62001fff42064281555f359062001fff015500");

    fn create_state_provider_with_beacon_root_contract() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(BEACON_ROOT_CONTRACT_CODE.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(BEACON_ROOT_CONTRACT_CODE.clone()),
            HashMap::new(),
        );

        db
    }

    fn executor_provider(chain_spec: Arc<ChainSpec>) -> EthExecutorProvider<EthEvmConfig> {
        EthExecutorProvider { chain_spec, evm_config: Default::default() }
    }

    #[test]
    fn eip_4788_non_genesis_call() {
        let mut header =
            Header { timestamp: 1, number: 1, excess_blob_gas: Some(0), ..Header::default() };

        let db = create_state_provider_with_beacon_root_contract();

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // attempt to execute a block without parent beacon block root, expect err
        let err = provider
            .executor(StateProviderDatabase::new(&db))
            .execute(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect_err(
                "Executing cancun block without parent beacon block root field should fail",
            );

        assert_eq!(
            err.as_validation().unwrap().clone(),
            BlockValidationError::MissingParentBeaconBlockRoot
        );

        // fix header, set a gas limit
        header.parent_beacon_block_root = Some(B256::with_last_byte(0x69));

        let mut executor = provider.executor(StateProviderDatabase::new(&db));

        // Now execute a block with the fixed header, ensure that it does not fail
        executor
            .execute_and_verify(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .unwrap();

        // check the actual storage of the contract - it should be:
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH should be
        // header.timestamp
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH + HISTORY_BUFFER_LENGTH
        //   // should be parent_beacon_block_root
        let history_buffer_length = 8191u64;
        let timestamp_index = header.timestamp % history_buffer_length;
        let parent_beacon_block_root_index =
            timestamp_index % history_buffer_length + history_buffer_length;

        // get timestamp storage and compare
        let timestamp_storage =
            executor.state.storage(BEACON_ROOTS_ADDRESS, U256::from(timestamp_index)).unwrap();
        assert_eq!(timestamp_storage, U256::from(header.timestamp));

        // get parent beacon block root storage and compare
        let parent_beacon_block_root_storage = executor
            .state
            .storage(BEACON_ROOTS_ADDRESS, U256::from(parent_beacon_block_root_index))
            .expect("storage value should exist");
        assert_eq!(parent_beacon_block_root_storage, U256::from(0x69));
    }

    #[test]
    fn eip_4788_no_code_cancun() {
        // This test ensures that we "silently fail" when cancun is active and there is no code at
        // // BEACON_ROOTS_ADDRESS
        let header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            excess_blob_gas: Some(0),
            ..Header::default()
        };

        let db = StateProviderTest::default();

        // DON'T deploy the contract at genesis
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // attempt to execute an empty block with parent beacon block root, this should not fail
        provider
            .executor(StateProviderDatabase::new(&db))
            .execute(
                (
                    &BlockWithSenders {
                        block: Block { header, body: vec![], ommers: vec![], withdrawals: None },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while cancun is active should not fail",
            );
    }

    #[test]
    fn eip_4788_empty_account_call() {
        // This test ensures that we do not increment the nonce of an empty SYSTEM_ADDRESS account
        // // during the pre-block call

        let mut db = create_state_provider_with_beacon_root_contract();

        // insert an empty SYSTEM_ADDRESS
        db.insert_account(SYSTEM_ADDRESS, Account::default(), None, HashMap::new());

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // construct the header for block one
        let header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            excess_blob_gas: Some(0),
            ..Header::default()
        };

        let mut executor = provider.executor(StateProviderDatabase::new(&db));

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute_and_verify(
                &BlockWithSenders {
                    block: Block { header, body: vec![], ommers: vec![], withdrawals: None },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .expect(
                "Executing a block with no transactions while cancun is active should not fail",
            );

        // ensure that the nonce of the system address account has not changed
        let nonce = executor.state_mut().basic(SYSTEM_ADDRESS).unwrap().unwrap().nonce;
        assert_eq!(nonce, 0);
    }

    #[test]
    fn eip_4788_genesis_call() {
        let db = create_state_provider_with_beacon_root_contract();

        // activate cancun at genesis
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut header = chain_spec.genesis_header();

        let provider = executor_provider(chain_spec);

        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        // attempt to execute the genesis block with non-zero parent beacon block root, expect err
        header.parent_beacon_block_root = Some(B256::with_last_byte(0x69));
        let _err = executor
            .execute_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect_err(
                "Executing genesis cancun block with non-zero parent beacon block root field
    should fail",
            );

        // fix header
        header.parent_beacon_block_root = Some(B256::ZERO);

        // now try to process the genesis block again, this time ensuring that a system contract
        // call does not occur
        executor
            .execute_one(
                (
                    &BlockWithSenders {
                        block: Block { header, body: vec![], ommers: vec![], withdrawals: None },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .unwrap();

        // there is no system contract call so there should be NO STORAGE CHANGES
        // this means we'll check the transition state
        let transition_state = executor
            .state_mut()
            .transition_state
            .take()
            .expect("the evm should be initialized with bundle updates");

        // assert that it is the default (empty) transition state
        assert_eq!(transition_state, TransitionState::default());
    }

    #[test]
    fn eip_4788_high_base_fee() {
        // This test ensures that if we have a base fee, then we don't return an error when the
        // system contract is called, due to the gas price being less than the base fee.
        let header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            base_fee_per_gas: Some(u64::MAX),
            excess_blob_gas: Some(0),
            ..Header::default()
        };

        let db = create_state_provider_with_beacon_root_contract();

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);

        // execute header
        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        // Now execute a block with the fixed header, ensure that it does not fail
        executor
            .execute_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .unwrap();

        // check the actual storage of the contract - it should be:
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH should be
        // header.timestamp
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH + HISTORY_BUFFER_LENGTH
        //   // should be parent_beacon_block_root
        let history_buffer_length = 8191u64;
        let timestamp_index = header.timestamp % history_buffer_length;
        let parent_beacon_block_root_index =
            timestamp_index % history_buffer_length + history_buffer_length;

        // get timestamp storage and compare
        let timestamp_storage = executor
            .state_mut()
            .storage(BEACON_ROOTS_ADDRESS, U256::from(timestamp_index))
            .unwrap();
        assert_eq!(timestamp_storage, U256::from(header.timestamp));

        // get parent beacon block root storage and compare
        let parent_beacon_block_root_storage = executor
            .state_mut()
            .storage(BEACON_ROOTS_ADDRESS, U256::from(parent_beacon_block_root_index))
            .unwrap();
        assert_eq!(parent_beacon_block_root_storage, U256::from(0x69));
    }
}
