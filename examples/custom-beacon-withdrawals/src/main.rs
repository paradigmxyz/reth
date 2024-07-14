//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
use reth::{
    api::ConfigureEvm,
    builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    primitives::{address, Address, Bytes},
    providers::{ExecutionOutcome, ProviderError},
    revm::{
        batch::{BlockBatchRecord, BlockExecutorStats},
        db::states::bundle_state::BundleRetention,
        interpreter::Host,
        primitives::{BlockEnv, CfgEnvWithHandlerCfg, Env, EnvWithHandlerCfg, TransactTo, TxEnv},
        Database, DatabaseCommit, Evm, State,
    },
};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_ethereum_consensus::validate_block_post_execution;
use reth_evm::execute::{
    BatchExecutor, BlockExecutionError, BlockExecutionInput, BlockExecutionOutput,
    BlockExecutorProvider, Executor,
};
use reth_evm_ethereum::{
    execute::{EthEvmExecutor, EthExecuteOutput},
    EthEvmConfig,
};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{BlockNumber, BlockWithSenders, Header, Receipt, Receipts, Withdrawal, U256};
use reth_prune_types::PruneModes;
use std::fmt::Display;
use std::sync::Arc;

pub const SYSTEM_ADDRESS: Address = address!("fffffffffffffffffffffffffffffffffffffffe");
pub const WITHDRAWALS_ADDRESS: Address = address!("4200000000000000000000000000000000000000");

fn main() {
    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                // use the default ethereum node types
                .with_types::<EthereumNode>()
                // Configure the components of the node
                // use default ethereum components but use our custom pool
                .with_components(
                    EthereumNode::components().executor(CustomExecutorBuilder::default()),
                )
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// A custom executor builder
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CustomExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for CustomExecutorBuilder
where
    Node: FullNodeTypes,
{
    type EVM = EthEvmConfig;
    type Executor = CustomExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let chain_spec = ctx.chain_spec();

        let evm_config = EthEvmConfig::default();
        let executor = CustomExecutorProvider::new(chain_spec, evm_config);

        Ok((evm_config, executor))
    }
}

#[derive(Debug, Clone)]
pub struct CustomExecutorProvider<EvmConfig: Clone> {
    chain_spec: Arc<ChainSpec>,
    evm_config: EvmConfig,
}

impl<EvmConfig: Clone> CustomExecutorProvider<EvmConfig> {
    /// Creates a new executor provider.
    pub fn new(chain_spec: Arc<ChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, evm_config }
    }
}

impl<EvmConfig: Clone> CustomExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    fn custom_executor<DB>(&self, db: DB) -> CustomBlockExecutor<EvmConfig, DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        CustomBlockExecutor::new(
            self.chain_spec.clone(),
            self.evm_config.clone(),
            State::builder().with_database(db).with_bundle_update().without_state_clear().build(),
        )
    }
}

#[derive(Debug)]
pub struct CustomBlockExecutor<EvmConfig, DB> {
    /// Chain specific evm config that's used to execute a block.
    executor: EthEvmExecutor<EvmConfig>,
    /// The state to use for execution
    state: State<DB>,
}

impl<EvmConfig, DB> CustomBlockExecutor<EvmConfig, DB> {
    /// Creates a new Custom block executor.
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

// Trait required by ExecutorBuilder
// [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthExecutorProvider
impl<EvmConfig: Clone> BlockExecutorProvider for CustomExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> =
        CustomBlockExecutor<EvmConfig, DB>;
    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> =
        CustomBatchExecutor<EvmConfig, DB>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        self.custom_executor(db)
    }

    fn batch_executor<DB>(&self, db: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        let executor = self.custom_executor(db);
        CustomBatchExecutor {
            executor,
            batch_record: BlockBatchRecord::default(),
            stats: BlockExecutorStats::default(),
        }
    }
}

impl<EvmConfig, DB> CustomBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_env_for_block(&self, header: &Header, total_difficulty: U256) -> EnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.executor.evm_config.fill_cfg_and_block_env(
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
            self.executor.execute_state_transitions(block, evm)
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
        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let mut evm = self.executor.evm_config.evm_with_env(&mut self.state, env);

        if let Some(withdrawals) = block.withdrawals.as_ref() {
            apply_withdrawals_contract_call(withdrawals, &mut evm)?;
        }

        Ok(())
    }
}

impl<EvmConfig, DB> Executor<DB> for CustomBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;
        let EthExecuteOutput { receipts, requests, gas_used } =
            self.execute_without_verification(block, total_difficulty)?;

        // NOTE: we need to merge keep the reverts for the bundle retention
        self.state.merge_transitions(BundleRetention::Reverts);

        Ok(BlockExecutionOutput { state: self.state.take_bundle(), receipts, requests, gas_used })
    }
}

/// An executor for a batch of blocks.
///
/// State changes are tracked until the executor is finalized.
#[derive(Debug)]
// [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
pub struct CustomBatchExecutor<EvmConfig, DB> {
    /// The executor used to execute blocks.
    executor: CustomBlockExecutor<EvmConfig, DB>,
    /// Keeps track of the batch and record receipts based on the configured prune mode
    batch_record: BlockBatchRecord,
    stats: BlockExecutorStats,
}

// [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
impl<EvmConfig, DB> CustomBatchExecutor<EvmConfig, DB> {
    /// Returns the receipts of the executed blocks.
    pub const fn receipts(&self) -> &Receipts {
        self.batch_record.receipts()
    }

    /// Returns mutable reference to the state that wraps the underlying database.
    #[allow(unused)]
    fn state_mut(&mut self) -> &mut State<DB> {
        self.executor.state_mut()
    }
}

// [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
impl<EvmConfig, DB> BatchExecutor<DB> for CustomBatchExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    // [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
    fn execute_and_verify_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;
        let EthExecuteOutput { receipts, requests, gas_used: _ } =
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

    // [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
    fn finalize(mut self) -> Self::Output {
        self.stats.log_debug();

        ExecutionOutcome::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
            self.batch_record.take_requests(),
        )
    }

    // [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
    fn set_tip(&mut self, tip: BlockNumber) {
        self.batch_record.set_tip(tip);
    }

    // [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.batch_record.set_prune_modes(prune_modes);
    }

    // [Custom/fork] Copy paste code from crates/ethereum/evm/src/execute.rs::EthBatchExecutor
    fn size_hint(&self) -> Option<usize> {
        Some(self.executor.state.bundle_state.size_hint())
    }
}

sol!(
    function withdrawals(
        uint64[] calldata amounts,
        address[] calldata addresses
    );
);

/// Applies the post-block call to the withdrawal / deposit contract, using the given block,
/// [`ChainSpec`], EVM.
pub fn apply_withdrawals_contract_call<EXT, DB: Database + DatabaseCommit>(
    withdrawals: &[Withdrawal],
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<(), BlockExecutionError>
where
    DB::Error: std::fmt::Display,
{
    // get previous env
    let previous_env = Box::new(evm.context.env().clone());

    // modify env for pre block call
    fill_tx_env_with_system_contract_call(
        &mut evm.context.evm.env,
        SYSTEM_ADDRESS,
        WITHDRAWALS_ADDRESS,
        withdrawalsCall {
            amounts: withdrawals.iter().map(|w| w.amount).collect::<Vec<_>>(),
            addresses: withdrawals.iter().map(|w| w.address).collect::<Vec<_>>(),
        }
        .abi_encode()
        .into(),
    );

    let mut state = match evm.transact() {
        Ok(res) => res.state,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockExecutionError::Other(
                format!("withdrawal contract system call revert: {}", e).into(),
            ));
        }
    };

    // Clean-up post system tx context
    state.remove(&SYSTEM_ADDRESS);
    state.remove(&evm.block().coinbase);
    evm.context.evm.db.commit(state);
    // re-set the previous env
    evm.context.evm.env = previous_env;

    Ok(())
}

fn fill_tx_env_with_system_contract_call(
    env: &mut Env,
    caller: Address,
    contract: Address,
    data: Bytes,
) {
    env.tx = TxEnv {
        caller,
        transact_to: TransactTo::Call(contract),
        // Explicitly set nonce to None so revm does not do any nonce checks
        nonce: None,
        gas_limit: 30_000_000,
        value: U256::ZERO,
        data,
        // Setting the gas price to zero enforces that no value is transferred as part of the call,
        // and that the call will not count against the block's gas limit
        gas_price: U256::ZERO,
        // The chain ID check is not relevant here and is disabled if set to None
        chain_id: None,
        // Setting the gas priority fee to None ensures the effective gas price is derived from the
        // `gas_price` field, which we need to be zero
        gas_priority_fee: None,
        access_list: Vec::new(),
        // blob fields can be None for this tx
        blob_hashes: Vec::new(),
        max_fee_per_blob_gas: None,
        authorization_list: None,
    };

    // ensure the block gas limit is >= the tx
    env.block.gas_limit = U256::from(env.tx.gas_limit);

    // disable the base fee check for this call by setting the base fee to zero
    env.block.basefee = U256::ZERO;
}
