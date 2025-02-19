//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_consensus::BlockHeader;
use alloy_eips::eip4895::Withdrawal;
use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
use reth::{
    api::{ConfigureEvm, NodeTypesWithEngine},
    builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    providers::BlockExecutionResult,
    revm::{
        db::State,
        primitives::{address, Address},
        DatabaseCommit,
    },
};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_evm::{
    execute::{
        BlockExecutionError, BlockExecutionStrategy, BlockExecutionStrategyFactory,
        InternalBlockExecutionError,
    },
    Database, Evm,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_ethereum::{node::EthereumAddOns, BasicBlockExecutorProvider, EthereumNode};
use reth_primitives::{
    Block, EthPrimitives, Receipt, Recovered, RecoveredBlock, SealedBlock, TransactionSigned,
};
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

    fn create_strategy<'a, DB>(
        &'a mut self,
        db: &'a mut State<DB>,
        block: &'a RecoveredBlock<Block>,
    ) -> impl BlockExecutionStrategy<Primitives = Self::Primitives, Error = BlockExecutionError> + 'a
    where
        DB: Database,
    {
        let evm = self.evm_config.evm_for_block(db, block.header());
        CustomExecutorStrategy { evm, factory: self, block }
    }
}

pub struct CustomExecutorStrategy<'a, Evm> {
    /// Reference to the parent factory.
    factory: &'a CustomExecutorStrategyFactory,
    /// EVM used for execution.
    evm: Evm,
    /// Block being executed.
    block: &'a SealedBlock,
}

impl<'db, DB, E> BlockExecutionStrategy for CustomExecutorStrategy<'_, E>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>>,
{
    type Primitives = EthPrimitives;
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(&mut self) -> Result<(), Self::Error> {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            (*self.factory.chain_spec).is_spurious_dragon_active_at_block(self.block.number());
        self.evm.db_mut().set_state_clear_flag(state_clear_flag);

        Ok(())
    }

    fn execute_transactions<'a>(
        &mut self,
        _transactions: impl IntoIterator<Item = Recovered<&'a TransactionSigned>>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn apply_post_execution_changes(
        mut self,
    ) -> Result<BlockExecutionResult<Receipt>, Self::Error> {
        if let Some(withdrawals) = self.block.body().withdrawals.as_ref() {
            apply_withdrawals_contract_call(withdrawals, &mut self.evm)?;
        }

        Ok(Default::default())
    }

    fn with_state_hook(&mut self, _hook: Option<Box<dyn reth_evm::system_calls::OnStateHook>>) {}
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
    state.remove(&evm.block().beneficiary);

    evm.db_mut().commit(state);

    Ok(())
}
