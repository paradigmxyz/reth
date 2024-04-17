//! Ethereum executor.

use std::sync::Arc;

use revm_primitives::{
    BlockEnv,
    CfgEnvWithHandlerCfg, db::{Database, DatabaseCommit}, EnvWithHandlerCfg, ResultAndState,
};

use reth_evm::{
    ConfigureEvm,
    ConfigureEvmEnv, execute::{BatchBlockOutput, BatchExecutor, EthBlockExecutionInput, EthBlockOutput, Executor},
};
use reth_interfaces::executor::{BlockExecutionError, BlockValidationError};
use reth_primitives::{
    BlockWithSenders, ChainSpec, GotExpected, Header, Receipt, Receipts, U256,
};
use reth_provider::BundleStateWithReceipts;
use reth_revm::{
    batch::{BlockBatchRecord, BlockExecutorStats}
    stack::InspectorStack,
    State
    ,state_change::apply_beacon_root_contract_call,
};

/// A basic Ethereum block executor.
///
/// Expected usage:
/// - Create a new instance of the executor.
/// - Execute the block.
#[derive(Debug)]
pub struct EthBlockExecutor<EvmConfig, DB> {
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
    /// The state to use for execution
    state: State<DB>,
    /// Optional inspector stack for debugging
    inspector: Option<InspectorStack>,
}

impl<EvmConfig, DB> EthBlockExecutor<EvmConfig, DB> {
    /// Creates a new Ethereum block executor.
    pub fn new(chain_spec: Arc<ChainSpec>, evm: EvmConfig, state: State<DB>) -> Self {
        Self { chain_spec, evm_config: evm, state, inspector: None }
    }

    /// Sets the inspector stack for debugging.
    pub fn with_inspector(mut self, inspector: InspectorStack) -> Self {
        self.inspector = Some(inspector);
        self
    }
}

impl<EvmConfig, DB> EthBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
   // TODO: get rid of this
    EvmConfig: ConfigureEvmEnv<TxMeta = ()>,
    DB: Database + DatabaseCommit,
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
            &self.chain_spec,
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
        let mut evm = self.evm_config.evm_with_env(&mut self.state, env);

        // 3. apply pre execution changes
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

            EvmConfig::fill_tx_env(evm.tx_mut(), &transaction, *sender, ());

            // Execute transaction.
            let ResultAndState { result, state } = evm.transact().map_err(move |err| {
                // // Ensure hash is calculated for error log, if not already done
                // BlockValidationError::EVM {
                //     hash: transaction.recalculate_hash(),
                //     error: err.into(),
                // }
                // .into()
                todo!()
            })?;
            // self.stats.execution_duration += time.elapsed();
            // let time = Instant::now();

            evm.db_mut().commit(state);

            // self.stats.apply_state_duration += time.elapsed();

            // append gas used
            cumulative_gas_used += result.gas_used();

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                // Success flag was added in `EIP-658: Embedding transaction status code in
                // receipts`.
                success: result.is_success(),
                cumulative_gas_used,
                // convert to reth log
                logs: result.into_logs().into_iter().map(Into::into).collect(),

                ..Default::default()
            });
        }

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            let receipts = Receipts::from_block_receipt(receipts);
            return Err(BlockValidationError::BlockGasUsed {
                gas: GotExpected { got: cumulative_gas_used, expected: block.gas_used },
                gas_spent_by_tx: receipts.gas_spent_by_tx()?,
            }
            .into())
        }

        // 5. apply post execution changes

        // TODO Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is required for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec.is_byzantium_active_at_block(block.header.number) {
            // if let Err(error) =
            //     verify_receipt(block.header.receipts_root, block.header.logs_bloom,
            // receipts.iter()) {
            //     debug!(target: "evm", %error, ?receipts, "receipts verification failed");
            //     return Err(error)
            // };
        }

        todo!()
    }

    fn execute_transactions(&mut self) {}

    /// Apply settings before a new block is executed.
    pub(crate) fn on_new_block(&mut self, header: &Header) {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag = self.chain_spec.is_spurious_dragon_active_at_block(header.number);
        self.state.set_state_clear_flag(state_clear_flag);
    }

    /// Invoked before transactions are executed
    fn pre_execution(&self, block: &BlockWithSenders) -> Result<(), BlockExecutionError> {
        todo!()
    }

    /// Apply post execution state changes, including block rewards, withdrawals, and irregular DAO
    /// hardfork state change.
    fn post_execution(&self) {
        todo!()
    }
}

impl<EvmConfig, DB> Executor for EthBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    EvmConfig: ConfigureEvmEnv<TxMeta = ()>,
    DB: Database + DatabaseCommit,
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
        Ok(EthBlockOutput { state: self.state.take_bundle(), receipts, gas_used })
    }
}

/// An executor for a batch of blocks.
///
/// State changes are tracked until the executor is finalized.
#[derive(Debug)]
pub struct EthBatchExecutor<EvmConfig, DB> {
    /// The executor used to execute blocks.
    executor: EthBlockExecutor<EvmConfig, DB>,
    /// Keeps track of the batch and record receipts based on the configured prune mode
    batch_record: BlockBatchRecord,
    stats: BlockExecutorStats,
}

impl<EvmConfig, DB> BatchExecutor for EthBatchExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    // TODO: get rid of this
    EvmConfig: ConfigureEvmEnv<TxMeta = ()>,
    DB: Database + DatabaseCommit,
{
    type Input<'a> = EthBlockExecutionInput<'a, BlockWithSenders>;
    type Output = BundleStateWithReceipts;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<BatchBlockOutput, Self::Error> {
        let EthBlockExecutionInput { block, total_difficulty } = input;
        let (receipts, _gas_used) = self.executor.execute_and_verify(block, total_difficulty)?;
        self.batch_record.save_receipts(receipts)?;

        Ok(BatchBlockOutput { size_hint: Some(self.executor.state.bundle_size_hint()) })
    }

    fn finalize(mut self) -> Self::Output {
        self.stats.log_debug();

        BundleStateWithReceipts::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
        )
    }
}
