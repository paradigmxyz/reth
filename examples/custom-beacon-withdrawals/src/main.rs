//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_eips::{eip4895::Withdrawal, eip7685::Requests};
use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
#[cfg(feature = "optimism")]
use reth::revm::primitives::OptimismFields;
use reth::{
    api::{ConfigureEvm, ConfigureEvmEnv, NodeTypesWithEngine},
    builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    providers::ProviderError,
    revm::{
        interpreter::Host,
        primitives::{
            address, Address, BlockEnv, Bytes, CfgEnvWithHandlerCfg, Env, EnvWithHandlerCfg,
            TransactTo, TxEnv, U256,
        },
        Database, DatabaseCommit, Evm, State,
    },
};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_evm::execute::{
    BlockExecutionError, BlockExecutionStrategy, BlockExecutionStrategyFactory, ExecuteOutput,
    InternalBlockExecutionError,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_ethereum::{node::EthereumAddOns, BasicBlockExecutorProvider, EthereumNode};
use reth_primitives::{BlockWithSenders, Receipt};
use std::{fmt::Display, sync::Arc};

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
                .with_add_ons(EthereumAddOns::default())
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

impl<Types, Node> ExecutorBuilder<Node> for CustomExecutorBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = EthEvmConfig;
    type Executor = BasicBlockExecutorProvider<CustomExecutorStrategyFactory>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let chain_spec = ctx.chain_spec();
        let evm_config = EthEvmConfig::new(ctx.chain_spec());
        let strategy_factory =
            CustomExecutorStrategyFactory { chain_spec, evm_config: evm_config.clone() };
        let executor = BasicBlockExecutorProvider::new(strategy_factory);

        Ok((evm_config, executor))
    }
}

#[derive(Clone)]
pub struct CustomExecutorStrategyFactory {
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: EthEvmConfig,
}

impl BlockExecutionStrategyFactory for CustomExecutorStrategyFactory {
    type Strategy<DB: Database<Error: Into<ProviderError> + Display>> = CustomExecutorStrategy<DB>;

    fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        let state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        CustomExecutorStrategy {
            state,
            chain_spec: self.chain_spec.clone(),
            evm_config: self.evm_config.clone(),
        }
    }
}

pub struct CustomExecutorStrategy<DB>
where
    DB: Database<Error: Into<ProviderError> + Display>,
{
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: EthEvmConfig,
    /// Current state for block execution.
    state: State<DB>,
}

impl<DB> CustomExecutorStrategy<DB>
where
    DB: Database<Error: Into<ProviderError> + Display>,
{
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_env_for_block(
        &self,
        header: &alloy_consensus::Header,
        total_difficulty: U256,
    ) -> EnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.evm_config.fill_cfg_and_block_env(&mut cfg, &mut block_env, header, total_difficulty);

        EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default())
    }
}

impl<DB> BlockExecutionStrategy<DB> for CustomExecutorStrategy<DB>
where
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        _total_difficulty: U256,
    ) -> Result<(), Self::Error> {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            (*self.chain_spec).is_spurious_dragon_active_at_block(block.header.number);
        self.state.set_state_clear_flag(state_clear_flag);

        Ok(())
    }

    fn execute_transactions(
        &mut self,
        _block: &BlockWithSenders,
        _total_difficulty: U256,
    ) -> Result<ExecuteOutput, Self::Error> {
        Ok(ExecuteOutput { receipts: vec![], gas_used: 0 })
    }

    fn apply_post_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
        _receipts: &[Receipt],
    ) -> Result<Requests, Self::Error> {
        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let mut evm = self.evm_config.evm_with_env(&mut self.state, env);

        if let Some(withdrawals) = block.body.withdrawals.as_ref() {
            apply_withdrawals_contract_call(withdrawals, &mut evm)?;
        }

        Ok(Requests::default())
    }

    fn state_ref(&self) -> &State<DB> {
        &self.state
    }

    fn state_mut(&mut self) -> &mut State<DB> {
        &mut self.state
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
            return Err(BlockExecutionError::Internal(InternalBlockExecutionError::Other(
                format!("withdrawal contract system call revert: {}", e).into(),
            )))
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
        #[cfg(feature = "optimism")]
        optimism: OptimismFields::default(),
    };

    // ensure the block gas limit is >= the tx
    env.block.gas_limit = U256::from(env.tx.gas_limit);

    // disable the base fee check for this call by setting the base fee to zero
    env.block.basefee = U256::ZERO;
}
