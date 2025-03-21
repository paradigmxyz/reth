//! Ethereum block executor.

use crate::{
    dao_fork::{DAO_HARDFORK_BENEFICIARY, DAO_HARDKFORK_ACCOUNTS},
    taiko::{check_anchor_tx, check_anchor_tx_ontake, check_anchor_tx_pacaya, TaikoData},
    EthEvmConfig,
};
use reth_chainspec::{ChainSpec, MAINNET};
pub use reth_consensus::Consensus;
pub use reth_ethereum_consensus::{EthBeaconConsensus, validate_block_post_execution};
use reth_evm::{
    execute::{
        BatchExecutor, BlockExecutionError, BlockExecutionInput, BlockExecutionOutput,
        BlockExecutorProvider, BlockValidationError, Executor, ProviderError,
    },
    ConfigureEvm,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    revm::config::revm_spec, BlockNumber, BlockWithSenders, Hardfork, Head, Header, Receipt,
    Request, Withdrawals, U256,
};
use reth_prune_types::PruneModes;
use reth_revm::{
    batch::{BlockBatchRecord, BlockExecutorStats},
    db::states::bundle_state::BundleRetention,
    state_change::{
        apply_beacon_root_contract_call, apply_blockhashes_update,
        apply_withdrawal_requests_contract_call, post_block_balance_increments,
    },
    Evm, State,
    JournaledState,
};
use revm_primitives::{
    db::{Database, DatabaseCommit}, Address, BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ResultAndState,
    EVMError, HashSet, SpecId,
};
use std::sync::Arc;
use anyhow::Result;
use tracing::{debug, warn};

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
    pub const fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, evm_config }
    }
}

impl<EvmConfig> EthExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    /// Creates an Ethereum block executor
    pub fn eth_executor<DB>(&self, db: DB) -> EthBlockExecutor<EvmConfig, DB>
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

/// Helper type for the output of executing a block.
#[derive(Debug, Clone)]
struct EthExecuteOutput {
    receipts: Vec<Receipt>,
    requests: Vec<Request>,
    gas_used: u64,
    valid_transaction_indices: Vec<usize>,
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
    /// Executes the transactions in the block and returns the receipts of the transactions in the
    /// block, the total gas used and the list of EIP-7685 [requests](Request).
    ///
    /// This applies the pre-execution and post-execution changes that require an [EVM](Evm), and
    /// executes the transactions.
    ///
    /// # Note
    ///
    /// It does __not__ apply post-execution changes that do not require an [EVM](Evm), for that see
    /// [`EthBlockExecutor::post_execution`].
    fn execute_state_transitions<Ext, DB>(
        &self,
        block: &BlockWithSenders,
        mut evm: Evm<'_, Ext, &mut State<DB>>,
        optimistic: bool,
        taiko_data: Option<TaikoData>,
    ) -> Result<EthExecuteOutput, BlockExecutionError>
    where
        DB: Database<Error = ProviderError>,
    {
        let is_taiko = self.chain_spec.is_taiko();

        // apply pre execution changes
        apply_beacon_root_contract_call(
            &self.chain_spec,
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut evm,
        )?;
        apply_blockhashes_update(
            evm.db_mut(),
            &self.chain_spec,
            block.timestamp,
            block.number,
            block.parent_hash,
        )?;

        // execute transactions
        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        let mut valid_transaction_indices = Vec::new();
        for (idx, (sender, transaction)) in block.transactions_with_sender().enumerate() {
            let is_anchor = is_taiko && idx == 0;

            debug!("Executing {} tx {:?}", idx, transaction.hash);

            // verify the anchor tx
            if is_anchor {
                let spec_id = revm_spec(
                    &self.chain_spec,
                    Head { number: block.number, ..Default::default() },
                );
                if spec_id.is_enabled_in(SpecId::PACAYA) {
                    check_anchor_tx_pacaya(
                        transaction,
                        sender,
                        &block.block,
                        taiko_data.clone().unwrap(),
                    )
                    .map_err(|e| BlockExecutionError::CanonicalRevert { inner: e.to_string() })?;
                } else if spec_id.is_enabled_in(SpecId::ONTAKE) {
                    check_anchor_tx_ontake(
                        transaction,
                        sender,
                        &block.block,
                        taiko_data.clone().unwrap(),
                    )
                    .map_err(|e| BlockExecutionError::CanonicalRevert { inner: e.to_string() })?;
                } else if spec_id.is_enabled_in(SpecId::HEKLA) {
                    check_anchor_tx(transaction, sender, &block.block, taiko_data.clone().unwrap())
                        .map_err(|e| BlockExecutionError::CanonicalRevert {
                            inner: e.to_string(),
                        })?;
                } else {
                    return Err(BlockExecutionError::CanonicalRevert {
                        inner: "unknown spec id for anchor".to_string(),
                    });
                }
            }

            // If the signature was not valid, the sender address will have been set to zero
            if *sender == Address::ZERO {
                // Signature can be invalid if not taiko or not the anchor tx
                if is_taiko && !is_anchor {
                    // If the signature is not valid, skip the transaction
                    continue;
                }
                // In all other cases, the tx needs to have a valid signature
                return Err(BlockExecutionError::CanonicalRevert { inner: "invalid tx".to_string() });
            }

            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                if optimistic {
                    continue;
                }
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            EvmConfig::fill_tx_env(evm.tx_mut(), transaction, *sender);

            // Set taiko specific data
            evm.tx_mut().taiko.is_anchor = is_anchor;
            // set the treasury address
            evm.tx_mut().taiko.treasury = taiko_data.clone().unwrap().l2_contract;
            evm.tx_mut().taiko.basefee_ratio = taiko_data.clone().unwrap().base_fee_config.sharing_pctg;

            // Execute transaction.
            let res = evm.transact().map_err(move |err| {
                // Ensure hash is calculated for error log, if not already done
                BlockValidationError::EVM {
                    hash: transaction.recalculate_hash(),
                    error: err.into(),
                }
            });
            if res.is_err() {
                // Clear the state for the next tx
                evm.context.evm.journaled_state = JournaledState::new(evm.context.evm.journaled_state.spec, HashSet::new());

                if optimistic {
                    match res {
                        Err(BlockValidationError::EVM { hash: _, error }) => match *error {
                            EVMError::Transaction(_invalid_transaction) => {}
                            _ => {
                                debug!("optimistic skipping tx due to evm error: {:?}", error);
                            }
                        },
                        _ => {
                            debug!("optimistic skipping tx due to other error: {:?}", &res);
                        }
                    }
                    continue;
                }

                if !is_taiko || is_anchor {
                    return Err(BlockExecutionError::Validation(res.err().unwrap()));
                }
                // only continue for invalid tx errors, not db errors (because those can be
                // manipulated by the prover)
                match res {
                    Err(BlockValidationError::EVM { hash, error }) => match *error {
                        EVMError::Transaction(invalid_transaction) => {
                            println!("Invalid tx at {}: {:?}", idx, invalid_transaction);
                            // skip the tx
                            continue;
                        },
                        _ => {
                            // any other error is not allowed
                            return Err(BlockExecutionError::Validation(BlockValidationError::EVM { hash, error }));
                        },
                    },
                    _ => {
                        // Any other type of error is not allowed
                        return Err(BlockExecutionError::Validation(res.err().unwrap()));
                    }
                }
            }
            let ResultAndState { result, state } = res?;
            // append gas used
            cumulative_gas_used += result.gas_used();
            if is_taiko {
                let mining_gas_limit = taiko_data.clone().unwrap().gas_limit;
                if cumulative_gas_used > mining_gas_limit {
                    warn!(
                        "mining gas limit exceeded: {} > {}",
                        cumulative_gas_used, mining_gas_limit
                    );
                    break;
                }
            }

            evm.db_mut().commit(state);

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

            // Add the tx to the list of valid transactions
            valid_transaction_indices.push(idx);
        }

        let requests = if self.chain_spec.is_prague_active_at_timestamp(block.timestamp) {
            // Collect all EIP-6110 deposits
            let deposit_requests =
                crate::eip6110::parse_deposits_from_receipts(&self.chain_spec, &receipts)?;

            // Collect all EIP-7685 requests
            let withdrawal_requests = apply_withdrawal_requests_contract_call(&mut evm)?;

            [deposit_requests, withdrawal_requests].concat()
        } else {
            vec![]
        };

        Ok(EthExecuteOutput { receipts, requests, gas_used: cumulative_gas_used, valid_transaction_indices })
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
    /// Allows the execution to continue even when a tx is invalid
    optimistic: bool,
    /// Taiko data
    taiko_data: Option<TaikoData>,
}

impl<EvmConfig, DB> EthBlockExecutor<EvmConfig, DB> {
    /// Creates a new Ethereum block executor.
    pub const fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig, state: State<DB>) -> Self {
        Self { executor: EthEvmExecutor { chain_spec, evm_config }, state, optimistic: false, taiko_data: None }
    }

    /// Optimistic execution
    pub fn taiko_data(mut self, taiko_data: TaikoData) -> Self {
        self.taiko_data = Some(taiko_data);
        self
    }

    /// Optimistic execution
    pub fn optimistic(mut self, optimistic: bool) -> Self {
        self.optimistic = optimistic;
        self
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
    /// Returns the receipts of the transactions in the block, the total gas used and the list of
    /// EIP-7685 [requests](Request).
    ///
    /// Returns an error if execution fails.
    fn execute_without_verification(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<EthExecuteOutput, BlockExecutionError> {
        // 1. prepare state on new block
        self.on_new_block(&block.header);

        // 2. configure the evm and execute
        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let output = {
            let evm = self.executor.evm_config.evm_with_env(&mut self.state, env);
            self.executor.execute_state_transitions(block, evm, self.optimistic, self.taiko_data.clone())
        }?;

        // 3. apply post execution changes
        self.post_execution(block, total_difficulty)?;

        Ok(output)
    }

    /// Apply settings before a new block is executed.
    pub(crate) fn on_new_block(&mut self, header: &Header) {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag = self.chain_spec().is_spurious_dragon_active_at_block(header.number);
        self.state.set_state_clear_flag(state_clear_flag);
    }

    /// Apply post execution state changes that do not require an [EVM](Evm), such as: block
    /// rewards, withdrawals, and irregular DAO hardfork state change
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
    type Output = BlockExecutionOutput<Receipt, DB>;
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
        let EthExecuteOutput { receipts, requests, gas_used, valid_transaction_indices } =
            self.execute_without_verification(block, total_difficulty)?;

        // NOTE: we need to merge keep the reverts for the bundle retention
        self.state.merge_transitions(BundleRetention::Reverts);

        Ok(BlockExecutionOutput { state: self.state.take_bundle(), receipts, requests, gas_used, db: self.state, valid_transaction_indices })
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
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;
        let EthExecuteOutput { receipts, requests, gas_used: _, valid_transaction_indices: _ } =
            self.executor.execute_without_verification(block, total_difficulty)?;

        validate_block_post_execution(block, self.executor.chain_spec(), &receipts, &requests)?;

        // prepare the state according to the prune mode
        let retention = self.batch_record.bundle_retention(block.number);
        self.executor.state.merge_transitions(retention);

        // store receipts in the set
        self.batch_record.save_receipts(receipts)?;

        // store requests in the set
        self.batch_record.save_requests(requests);

        if self.batch_record.first_block().is_none() {
            self.batch_record.set_first_block(block.number);
        }

        Ok(())
    }

    fn finalize(mut self) -> Self::Output {
        self.stats.log_debug();

        ExecutionOutcome::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
            self.batch_record.take_requests(),
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
    use alloy_eips::{
        eip2935::HISTORY_STORAGE_ADDRESS,
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE, SYSTEM_ADDRESS},
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    };
    use reth_chainspec::{ChainSpecBuilder, ForkCondition};
    use reth_primitives::{
        constants::{EMPTY_ROOT_HASH, ETH_TO_WEI},
        keccak256, public_key_to_address, Account, Block, Transaction, TxKind, TxLegacy, B256,
    };
    use reth_revm::{
        database::StateProviderDatabase, state_change::HISTORY_SERVE_WINDOW,
        test_utils::StateProviderTest, TransitionState,
    };
    use reth_testing_utils::generators::{self, sign_tx_with_key_pair};
    use revm_primitives::{b256, fixed_bytes, Bytes};
    use secp256k1::{Keypair, Secp256k1};
    use std::collections::HashMap;

    fn create_state_provider_with_beacon_root_contract() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(BEACON_ROOTS_CODE.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(BEACON_ROOTS_CODE.clone()),
            HashMap::new(),
        );

        db
    }

    fn create_state_provider_with_withdrawal_requests_contract() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let withdrawal_requests_contract_account = Account {
            nonce: 1,
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone())),
        };

        db.insert_account(
            WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            withdrawal_requests_contract_account,
            Some(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone()),
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
                            requests: None,
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
            .execute_without_verification(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                        requests: None,
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
            .batch_executor(StateProviderDatabase::new(&db), PruneModes::none())
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
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

        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
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
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
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
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
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
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header: header.clone(),
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
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

    fn create_state_provider_with_block_hashes(latest_block: u64) -> StateProviderTest {
        let mut db = StateProviderTest::default();
        for block_number in 0..=latest_block {
            db.insert_block_hash(block_number, keccak256(block_number.to_string()));
        }
        db
    }

    #[test]
    fn eip_2935_pre_fork() {
        let db = create_state_provider_with_block_hashes(1);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Prague, ForkCondition::Never)
                .build(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        // construct the header for block one
        let header = Header { timestamp: 1, number: 1, ..Header::default() };

        // attempt to execute an empty block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // ensure that the block hash was *not* written to storage, since this is before the fork
        // was activated
        //
        // we load the account first, which should also not exist, because revm expects it to be
        // loaded
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_none());
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_2935_fork_activation_genesis() {
        let db = create_state_provider_with_block_hashes(0);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        let header = chain_spec.genesis_header();
        let provider = executor_provider(chain_spec);
        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        // attempt to execute genesis block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // ensure that the block hash was *not* written to storage, since there are no blocks
        // preceding genesis
        //
        // we load the account first, which should also not exist, because revm expects it to be
        // loaded
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_none());
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_2935_fork_activation_within_window_bounds() {
        let fork_activation_block = HISTORY_SERVE_WINDOW - 10;
        let db = create_state_provider_with_block_hashes(fork_activation_block);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Prague, ForkCondition::Timestamp(1))
                .build(),
        );

        let header = Header {
            parent_hash: B256::random(),
            timestamp: 1,
            number: fork_activation_block,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };
        let provider = executor_provider(chain_spec);
        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        // attempt to execute the fork activation block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the hash for the ancestor of the fork activation block should be present
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor
                .state_mut()
                .storage(HISTORY_STORAGE_ADDRESS, U256::from(fork_activation_block - 1))
                .unwrap(),
            U256::ZERO
        );

        // the hash of the block itself should not be in storage
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::from(fork_activation_block))
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_2935_fork_activation_outside_window_bounds() {
        let fork_activation_block = HISTORY_SERVE_WINDOW + 256;
        let db = create_state_provider_with_block_hashes(fork_activation_block);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Prague, ForkCondition::Timestamp(1))
                .build(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        let header = Header {
            parent_hash: B256::random(),
            timestamp: 1,
            number: fork_activation_block,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };

        // attempt to execute the fork activation block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the hash for the ancestor of the fork activation block should be present
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor
                .state_mut()
                .storage(
                    HISTORY_STORAGE_ADDRESS,
                    U256::from(fork_activation_block % HISTORY_SERVE_WINDOW - 1)
                )
                .unwrap(),
            U256::ZERO
        );
    }

    #[test]
    fn eip_2935_state_transition_inside_fork() {
        let db = create_state_provider_with_block_hashes(2);

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut header = chain_spec.genesis_header();
        header.requests_root = Some(EMPTY_ROOT_HASH);
        let header_hash = header.hash_slow();

        let provider = executor_provider(chain_spec);
        let mut executor =
            provider.batch_executor(StateProviderDatabase::new(&db), PruneModes::none());

        // attempt to execute the genesis block, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // nothing should be written as the genesis has no ancestors
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_none());
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
            .unwrap()
            .is_zero());

        // attempt to execute block 1, this should not fail
        let header = Header {
            parent_hash: header_hash,
            timestamp: 1,
            number: 1,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };
        let header_hash = header.hash_slow();

        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the block hash of genesis should now be in storage, but not block 1
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor.state_mut().storage(HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap(),
            U256::ZERO
        );
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::from(1))
            .unwrap()
            .is_zero());

        // attempt to execute block 2, this should not fail
        let header = Header {
            parent_hash: header_hash,
            timestamp: 1,
            number: 2,
            requests_root: Some(EMPTY_ROOT_HASH),
            ..Header::default()
        };

        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: vec![],
                            ommers: vec![],
                            withdrawals: None,
                            requests: None,
                        },
                        senders: vec![],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect(
                "Executing a block with no transactions while Prague is active should not fail",
            );

        // the block hash of genesis and block 1 should now be in storage, but not block 2
        assert!(executor.state_mut().basic(HISTORY_STORAGE_ADDRESS).unwrap().is_some());
        assert_ne!(
            executor.state_mut().storage(HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap(),
            U256::ZERO
        );
        assert_ne!(
            executor.state_mut().storage(HISTORY_STORAGE_ADDRESS, U256::from(1)).unwrap(),
            U256::ZERO
        );
        assert!(executor
            .state_mut()
            .storage(HISTORY_STORAGE_ADDRESS, U256::from(2))
            .unwrap()
            .is_zero());
    }

    #[test]
    fn eip_7002() {
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut db = create_state_provider_with_withdrawal_requests_contract();

        let secp = Secp256k1::new();
        let sender_key_pair = Keypair::new(&secp, &mut generators::rng());
        let sender_address = public_key_to_address(sender_key_pair.public_key());

        db.insert_account(
            sender_address,
            Account { nonce: 1, balance: U256::from(ETH_TO_WEI), bytecode_hash: None },
            None,
            HashMap::new(),
        );

        // https://github.com/lightclient/7002asm/blob/e0d68e04d15f25057af7b6d180423d94b6b3bdb3/test/Contract.t.sol.in#L49-L64
        let validator_public_key = fixed_bytes!("111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111");
        let withdrawal_amount = fixed_bytes!("2222222222222222");
        let input: Bytes = [&validator_public_key[..], &withdrawal_amount[..]].concat().into();
        assert_eq!(input.len(), 56);

        let mut header = chain_spec.genesis_header();
        header.gas_limit = 1_500_000;
        header.gas_used = 134_807;
        header.receipts_root =
            b256!("b31a3e47b902e9211c4d349af4e4c5604ce388471e79ca008907ae4616bb0ed3");

        let tx = sign_tx_with_key_pair(
            sender_key_pair,
            Transaction::Legacy(TxLegacy {
                chain_id: Some(chain_spec.chain.id()),
                nonce: 1,
                gas_price: header.base_fee_per_gas.unwrap().into(),
                gas_limit: 134_807,
                to: TxKind::Call(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS),
                // `MIN_WITHDRAWAL_REQUEST_FEE`
                value: U256::from(1),
                input,
            }),
        );

        let provider = executor_provider(chain_spec);

        let executor = provider.executor(StateProviderDatabase::new(&db));

        let BlockExecutionOutput { receipts, requests, .. } = executor
            .execute(
                (
                    &Block {
                        header,
                        body: vec![tx],
                        ommers: vec![],
                        withdrawals: None,
                        requests: None,
                    }
                    .with_recovered_senders()
                    .unwrap(),
                    U256::ZERO,
                )
                    .into(),
            )
            .unwrap();

        let receipt = receipts.first().unwrap();
        assert!(receipt.success);

        let request = requests.first().unwrap();
        let withdrawal_request = request.as_withdrawal_request().unwrap();
        assert_eq!(withdrawal_request.source_address, sender_address);
        assert_eq!(withdrawal_request.validator_pubkey, validator_public_key);
        assert_eq!(withdrawal_request.amount, u64::from_be_bytes(withdrawal_amount.into()));
    }

    #[test]
    fn block_gas_limit_error() {
        // Create a chain specification with fork conditions set for Prague
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Prague, ForkCondition::Timestamp(0))
                .build(),
        );

        // Create a state provider with the withdrawal requests contract pre-deployed
        let mut db = create_state_provider_with_withdrawal_requests_contract();

        // Initialize Secp256k1 for key pair generation
        let secp = Secp256k1::new();
        // Generate a new key pair for the sender
        let sender_key_pair = Keypair::new(&secp, &mut generators::rng());
        // Get the sender's address from the public key
        let sender_address = public_key_to_address(sender_key_pair.public_key());

        // Insert the sender account into the state with a nonce of 1 and a balance of 1 ETH in Wei
        db.insert_account(
            sender_address,
            Account { nonce: 1, balance: U256::from(ETH_TO_WEI), bytecode_hash: None },
            None,
            HashMap::new(),
        );

        // Define the validator public key and withdrawal amount as fixed bytes
        let validator_public_key = fixed_bytes!("111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111");
        let withdrawal_amount = fixed_bytes!("2222222222222222");
        // Concatenate the validator public key and withdrawal amount into a single byte array
        let input: Bytes = [&validator_public_key[..], &withdrawal_amount[..]].concat().into();
        // Ensure the input length is 56 bytes
        assert_eq!(input.len(), 56);

        // Create a genesis block header with a specified gas limit and gas used
        let mut header = chain_spec.genesis_header();
        header.gas_limit = 1_500_000;
        header.gas_used = 134_807;
        header.receipts_root =
            b256!("b31a3e47b902e9211c4d349af4e4c5604ce388471e79ca008907ae4616bb0ed3");

        // Create a transaction with a gas limit higher than the block gas limit
        let tx = sign_tx_with_key_pair(
            sender_key_pair,
            Transaction::Legacy(TxLegacy {
                chain_id: Some(chain_spec.chain.id()),
                nonce: 1,
                gas_price: header.base_fee_per_gas.unwrap().into(),
                gas_limit: 2_500_000, // higher than block gas limit
                to: TxKind::Call(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS),
                value: U256::from(1),
                input,
            }),
        );

        // Create an executor from the state provider
        let executor = executor_provider(chain_spec).executor(StateProviderDatabase::new(&db));

        // Execute the block and capture the result
        let exec_result = executor.execute(
            (
                &Block {
                    header,
                    body: vec![tx],
                    ommers: vec![],
                    withdrawals: None,
                    requests: None,
                }
                .with_recovered_senders()
                .unwrap(),
                U256::ZERO,
            )
                .into(),
        );

        // Check if the execution result is an error and assert the specific error type
        match exec_result {
            Ok(_) => panic!("Expected block gas limit error"),
            Err(err) => assert_eq!(
                *err.as_validation().unwrap(),
                BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: 2_500_000,
                    block_available_gas: 1_500_000,
                }
            ),
        }
    }
}
