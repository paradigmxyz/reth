//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_consensus::BlockHeader;
use alloy_eips::{eip4895::Withdrawal, eip7685::Requests};
use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
use reth::{
    api::{ConfigureEvm, NodeTypesWithEngine},
    builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    providers::ProviderError,
    revm::{
        primitives::{address, Address},
        Database, DatabaseCommit, State,
    },
};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_evm::{
    execute::{
        BlockExecutionError, BlockExecutionStrategy, BlockExecutionStrategyFactory, ExecuteOutput,
        InternalBlockExecutionError,
    },
    Evm,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_ethereum::{node::EthereumAddOns, BasicBlockExecutorProvider, EthereumNode};
use reth_primitives::{EthPrimitives, Receipt, RecoveredBlock};
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
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
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
    type Primitives = EthPrimitives;
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

impl<DB> BlockExecutionStrategy for CustomExecutorStrategy<DB>
where
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type DB = DB;
    type Primitives = EthPrimitives;
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(
        &mut self,
        block: &RecoveredBlock<reth_primitives::Block>,
    ) -> Result<(), Self::Error> {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            (*self.chain_spec).is_spurious_dragon_active_at_block(block.number());
        self.state.set_state_clear_flag(state_clear_flag);

        Ok(())
    }

    fn execute_transactions(
        &mut self,
        _block: &RecoveredBlock<reth_primitives::Block>,
    ) -> Result<ExecuteOutput<Receipt>, Self::Error> {
        Ok(ExecuteOutput { receipts: vec![], gas_used: 0 })
    }

    fn apply_post_execution_changes(
        &mut self,
        block: &RecoveredBlock<reth_primitives::Block>,
        _receipts: &[Receipt],
    ) -> Result<Requests, Self::Error> {
        let mut evm = self.evm_config.evm_for_block(&mut self.state, block.header());

        if let Some(withdrawals) = block.body().withdrawals.as_ref() {
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
pub fn apply_withdrawals_contract_call(
    withdrawals: &[Withdrawal],
    evm: &mut impl Evm<Error: Display, DB: DatabaseCommit>,
) -> Result<(), BlockExecutionError> {
    let mut state = match evm.transact_system_call(
        SYSTEM_ADDRESS,
        WITHDRAWALS_ADDRESS,
        withdrawalsCall {
            amounts: withdrawals.iter().map(|w| w.amount).collect::<Vec<_>>(),
            addresses: withdrawals.iter().map(|w| w.address).collect::<Vec<_>>(),
        }
        .abi_encode()
        .into(),
    ) {
        Ok(res) => res.state,
        Err(e) => {
            return Err(BlockExecutionError::Internal(InternalBlockExecutionError::Other(
                format!("withdrawal contract system call revert: {}", e).into(),
            )))
        }
    };

    // Clean-up post system tx context
    state.remove(&SYSTEM_ADDRESS);
    state.remove(&evm.block().coinbase);

    evm.db_mut().commit(state);

    Ok(())
}
