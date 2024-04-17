//! Optimism block executor.

//! Ethereum block executor.

use std::sync::Arc;

use revm_primitives::{
    db::{Database, DatabaseCommit},
    BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ResultAndState,
};
use tracing::debug;

use reth_evm::{
    execute::{
        BatchBlockOutput, BatchExecutor, EthBlockExecutionInput, EthBlockOutput, Executor,
        ExecutorProvider,
    },
    ConfigureEvm, ConfigureEvmEnv,
};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    provider::ProviderError,
};
use reth_primitives::{
    BlockWithSenders, Bytes, ChainSpec, GotExpected, Hardfork, Header, PruneModes, Receipt,
    Receipts, Withdrawals, U256,
};
use reth_provider::BundleStateWithReceipts;
use reth_revm::{
    batch::{BlockBatchRecord, BlockExecutorStats},
    db::states::bundle_state::BundleRetention,
    eth_dao_fork::{DAO_HARDFORK_BENEFICIARY, DAO_HARDKFORK_ACCOUNTS},
    processor::verify_receipt,
    stack::InspectorStack,
    state_change::{apply_beacon_root_contract_call, post_block_balance_increments},
    Evm, State, StateBuilder,
};

/// Provides executors to execute regular ethereum blocks
#[derive(Debug, Clone)]
pub struct OpExecutorProvider<EvmConfig> {
    chain_spec: Arc<ChainSpec>,
    evm_config: EvmConfig,
    inspector: Option<InspectorStack>,
    prune_modes: PruneModes,
}

impl<EvmConfig> OpExecutorProvider<EvmConfig> {
    /// Creates a new executor provider.
    pub fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, evm_config, inspector: None, prune_modes: PruneModes::none() }
    }

    /// Configures an optional inspector stack for debugging.
    pub fn with_inspector(mut self, inspector: InspectorStack) -> Self {
        self.inspector = Some(inspector);
        self
    }

    /// Configures the prune modes for the executor.
    pub fn with_prune_modes(mut self, prune_modes: PruneModes) -> Self {
        self.prune_modes = prune_modes;
        self
    }
}

impl<EvmConfig> OpExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm,
    EvmConfig: ConfigureEvmEnv<TxMeta = ()>,
{
    fn eth_executor<DB>(&self, db: DB) -> EthBlockExecutor<EvmConfig, DB>
    where
        DB: Database<Error = ProviderError> + DatabaseCommit,
    {
        EthBlockExecutor::new(
            self.chain_spec.clone(),
            self.evm_config.clone(),
            State::builder().with_database(db).with_bundle_update().without_state_clear().build(),
        )
        .with_inspector(self.inspector.clone())
    }
}

impl<EvmConfig> ExecutorProvider for OpExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm,
    EvmConfig: ConfigureEvmEnv<TxMeta = ()>,
{
    fn batch_executor<DB>(&self, db: DB) -> impl BatchExecutor
    where
        DB: Database<Error = ProviderError> + DatabaseCommit,
    {
        let executor = self.eth_executor(db);
        OpBatchExecutor {
            executor,
            batch_record: BlockBatchRecord::new(self.prune_modes.clone()),
            stats: BlockExecutorStats::default(),
        }
    }

    fn executor<DB>(&self, db: DB) -> impl Executor
    where
        DB: Database<Error = ProviderError> + DatabaseCommit,
    {
        self.eth_executor(db)
    }
}

/// Helper container type for EVM with chain spec.
#[derive(Debug, Clone)]
struct OpEvmExecutor<EvmConfig> {
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
}

impl<EvmConfig> OpEvmExecutor<EvmConfig>
where
    EvmConfig: ConfigureEvm,
    EvmConfig: ConfigureEvmEnv<TxMeta = Bytes>,
{
    /// Executes the transactions in the block and returns the receipts.
    ///
    /// This applies the pre-execution changes, executes the transactions.
    ///
    /// It does __not__ apply post-execution changes.
    fn execute_pre_and_transactions<Ext, DB>(
        &mut self,
        block: &BlockWithSenders,
        mut evm: Evm<'_, Ext, DB>,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError>
    where
        DB: Database<Error = ProviderError> + DatabaseCommit,
    {
        //  apply pre execution changes
        apply_beacon_root_contract_call(
            &self.chain_spec,
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut evm,
        )?;

        // 4. execute transactions
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

            let mut buf = Vec::with_capacity(transaction.length_without_header());
            transaction.encode_enveloped(&mut buf);
            EvmConfig::fill_tx_env(evm.tx_mut(), transaction, *sender, buf.into());

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
    /// Chain specific evm config
    evm_spec: OpEvmExecutor<EvmConfig>,
    /// The state to use for execution
    state: State<DB>,
    /// Optional inspector stack for debugging
    inspector: Option<InspectorStack>,
}

impl<EvmConfig, DB> EthBlockExecutor<EvmConfig, DB> {
    /// Creates a new Ethereum block executor.
    pub fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig, state: State<DB>) -> Self {
        Self { evm_spec: OpEvmExecutor { chain_spec, evm_config }, state, inspector: None }
    }

    /// Sets the inspector stack for debugging.
    pub fn with_inspector(mut self, inspector: Option<InspectorStack>) -> Self {
        self.inspector = inspector;
        self
    }

    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        &self.evm_spec.chain_spec
    }
}

impl<EvmConfig, DB> EthBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    // TODO: get rid of this
    EvmConfig: ConfigureEvmEnv<TxMeta = ()>,
    DB: Database<Error = ProviderError> + DatabaseCommit,
{
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// Caution: this does not initialize the tx environment.
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
            if let Some(inspector) = self.inspector.as_mut() {
                let evm = self.evm_spec.evm_config.evm_with_env_and_inspector(
                    &mut self.state,
                    env,
                    inspector,
                );
                self.evm_spec.execute_pre_and_transactions(block, evm)?
            } else {
                let evm = self.evm_spec.evm_config.evm_with_env(&mut self.state, env);

                self.evm_spec.execute_pre_and_transactions(block, evm)?
            }
        };

        // 3. apply post execution changes
        self.post_execution(block, total_difficulty)?;

        // Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is required for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec().is_byzantium_active_at_block(block.header.number) {
            if let Err(error) =
                verify_receipt(block.header.receipts_root, block.header.logs_bloom, receipts.iter())
            {
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

impl<EvmConfig, DB> Executor for EthBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    EvmConfig: ConfigureEvmEnv<TxMeta = ()>,
    DB: Database<Error = ProviderError> + DatabaseCommit,
{
    type Input<'a> = EthBlockExecutionInput<'a, BlockWithSenders>;
    type Output = EthBlockOutput<Receipt>;
    type Error = BlockExecutionError;

    /// Executes the block and commits the state changes.
    ///
    /// Returns the receipts of the transactions in the block.
    ///
    /// Returns an error if the block could not be executed or failed verification.
    ///
    /// State changes are committed to the database.
    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let EthBlockExecutionInput { block, total_difficulty } = input;
        let (receipts, gas_used) = self.execute_and_verify(block, total_difficulty)?;

        // prepare the state for extraction
        self.state.merge_transitions(BundleRetention::PlainState);

        Ok(EthBlockOutput { state: self.state.take_bundle(), receipts, gas_used })
    }
}

/// An executor for a batch of blocks.
///
/// State changes are tracked until the executor is finalized.
#[derive(Debug)]
pub struct OpBatchExecutor<EvmConfig, DB> {
    /// The executor used to execute blocks.
    executor: EthBlockExecutor<EvmConfig, DB>,
    /// Keeps track of the batch and record receipts based on the configured prune mode
    batch_record: BlockBatchRecord,
    stats: BlockExecutorStats,
}

impl<EvmConfig, DB> BatchExecutor for OpBatchExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    // TODO: get rid of this
    EvmConfig: ConfigureEvmEnv<TxMeta = Bytes>,
    DB: Database<Error = ProviderError> + DatabaseCommit,
{
    type Input<'a> = EthBlockExecutionInput<'a, BlockWithSenders>;
    type Output = BundleStateWithReceipts;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<BatchBlockOutput, Self::Error> {
        let EthBlockExecutionInput { block, total_difficulty } = input;
        let (receipts, _gas_used) = self.executor.execute_and_verify(block, total_difficulty)?;

        // prepare the state according to the prune mode
        let retention = self.batch_record.bundle_retention(block.number);
        self.executor.state.merge_transitions(retention);

        // store receipts in the set
        self.batch_record.save_receipts(receipts)?;

        Ok(BatchBlockOutput { size_hint: Some(self.executor.state.bundle_size_hint()) })
    }

    fn finalize(mut self) -> Self::Output {
        // TODO: track stats
        // self.stats.log_debug();

        BundleStateWithReceipts::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
        )
    }
}
