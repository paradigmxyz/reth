//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_eips::eip4895::{Withdrawal, Withdrawals};
use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
use reth::{
    api::{ConfigureEvm, NodeTypesWithEngine},
    builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    providers::BlockExecutionResult,
    revm::{
        context::result::ExecutionResult,
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
    ConfigureEvmEnv, Database, Evm, EvmEnv, EvmFor, InspectorFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_ethereum::{node::EthereumAddOns, BasicBlockExecutorProvider, EthereumNode};
use reth_primitives::{
    EthPrimitives, Receipt, Recovered, SealedBlock, SealedHeader, TransactionSigned,
};
use std::fmt::Display;

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
    type EVM = CustomEvmConfig;
    type Executor = BasicBlockExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = CustomEvmConfig { inner: EthEvmConfig::new(ctx.chain_spec()) };
        let executor = BasicBlockExecutorProvider::new(evm_config.clone());

        Ok((evm_config, executor))
    }
}

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: EthEvmConfig,
}

impl ConfigureEvmEnv for CustomEvmConfig {
    type Error = <EthEvmConfig as ConfigureEvmEnv>::Error;
    type Header = <EthEvmConfig as ConfigureEvmEnv>::Header;
    type Spec = <EthEvmConfig as ConfigureEvmEnv>::Spec;
    type Transaction = <EthEvmConfig as ConfigureEvmEnv>::Transaction;
    type TxEnv = <EthEvmConfig as ConfigureEvmEnv>::TxEnv;

    fn evm_env(&self, header: &Self::Header) -> EvmEnv<Self::Spec> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes,
    ) -> Result<EvmEnv<Self::Spec>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }
}

impl ConfigureEvm for CustomEvmConfig {
    type EvmFactory = <EthEvmConfig as ConfigureEvm>::EvmFactory;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }
}

pub struct CustomExecutionCtx<'a> {
    withdrawals: Option<&'a Withdrawals>,
}

impl BlockExecutionStrategyFactory for CustomEvmConfig {
    type Primitives = EthPrimitives;
    type ExecutionCtx<'a> = CustomExecutionCtx<'a>;
    type Strategy<'a, DB: Database + 'a, I: InspectorFor<&'a mut State<DB>, Self> + 'a> =
        CustomExecutorStrategy<'a, EvmFor<Self, &'a mut State<DB>, I>>;

    fn context_for_block<'a>(&self, block: &'a SealedBlock) -> Self::ExecutionCtx<'a> {
        CustomExecutionCtx { withdrawals: block.body().withdrawals.as_ref() }
    }

    fn context_for_next_block<'a>(
        &self,
        _parent: &SealedHeader,
        attributes: NextBlockEnvAttributes<'a>,
    ) -> Self::ExecutionCtx<'a> {
        CustomExecutionCtx { withdrawals: attributes.withdrawals }
    }

    fn create_strategy<'a, DB, I>(
        &'a self,
        evm: EvmFor<Self, &'a mut State<DB>, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Strategy<'a, DB, I>
    where
        DB: Database,
        I: InspectorFor<&'a mut State<DB>, Self> + 'a,
    {
        CustomExecutorStrategy {
            evm,
            chain_spec: self.inner.chain_spec(),
            withdrawals: ctx.withdrawals,
        }
    }
}

pub struct CustomExecutorStrategy<'a, Evm> {
    /// Chainspec.
    chain_spec: &'a ChainSpec,
    /// EVM used for execution.
    evm: Evm,
    /// Block withdrawals.
    withdrawals: Option<&'a Withdrawals>,
}

impl<'db, DB, E> BlockExecutionStrategy for CustomExecutorStrategy<'_, E>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>>,
{
    type Primitives = EthPrimitives;
    type Error = BlockExecutionError;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), Self::Error> {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            self.chain_spec.is_spurious_dragon_active_at_block(self.evm.block().number);
        self.evm.db_mut().set_state_clear_flag(state_clear_flag);

        Ok(())
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        _tx: Recovered<&TransactionSigned>,
        _f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>),
    ) -> Result<u64, Self::Error> {
        Ok(0)
    }

    fn apply_post_execution_changes(
        mut self,
    ) -> Result<BlockExecutionResult<Receipt>, Self::Error> {
        if let Some(withdrawals) = self.withdrawals {
            apply_withdrawals_contract_call(withdrawals, &mut self.evm)?;
        }

        Ok(Default::default())
    }

    fn with_state_hook(&mut self, _hook: Option<Box<dyn reth_evm::system_calls::OnStateHook>>) {}

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
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
    state.remove(&evm.block().beneficiary);

    evm.db_mut().commit(state);

    Ok(())
}
