//! Example for modifying a block post-execution step. It credits beacon withdrawals with a
//! contract call instead of minting native tokens.

#![warn(unused_crate_dependencies)]

use std::{
    borrow::Cow,
    convert::Infallible,
    sync::{Arc, Mutex},
};

use alloy_consensus::Header;
use alloy_eips::{eip2718::Decodable2718, eip4895::Withdrawal};
use alloy_primitives::{address, Address, Bytes};
use alloy_sol_types::{sol, SolCall};
use evm2::{
    evm::{
        AccountChangeRef, BlockStateAccumulator, StateChangeSink, StateChangeSource, StorageChange,
        SystemTx,
    },
    BaseEvmTypes,
};
use reth_ethereum::{
    chainspec::ChainSpec,
    cli::interface::Cli,
    evm::{
        primitives::{
            BlockExecutionError, BlockExecutionOutput, BlockExecutor, BlockExecutorFactory,
            ConfigureEngineEvm, ConfigureEvm, ExecutableTxIterator, ExecutorTx, GasOutput,
            NextBlockEnvAttributes,
        },
        EthBlockAssembler, EthBlockExecutionCtx, EthBlockExecutor, EthBlockExecutorFactory,
        EthEvmConfig, EthEvmEnv, ExecutableRecoveredTx, RethEvmFactory, RethReceiptBuilder,
    },
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{components::ExecutorBuilder, BuilderContext},
        node::EthereumAddOns,
        EthereumNode,
    },
    primitives::{SealedBlock, SealedHeader, SignedTransaction},
    rpc::types::engine::ExecutionData,
    Block, EthPrimitives, Receipt, TransactionSigned,
};
use reth_execution_types::hashed_post_state_from_execution_state;
use reth_trie_common::{HashedPostState, KeccakKeyHasher};

pub const SYSTEM_ADDRESS: Address = address!("0xfffffffffffffffffffffffffffffffffffffffe");
pub const WITHDRAWALS_ADDRESS: Address = address!("0x4200000000000000000000000000000000000000");

type HashedStateHook = Arc<Mutex<Box<dyn FnMut(HashedPostState) + Send>>>;

fn main() {
    Cli::parse_args()
        .run(async move |builder, _| {
            let handle = builder
                .with_types::<EthereumNode>()
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

/// A custom executor builder that installs the withdrawal contract execution strategy.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CustomExecutorBuilder;

impl<Types, Node> ExecutorBuilder<Node> for CustomExecutorBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = CustomEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let inner = EthEvmConfig::new(ctx.chain_spec());
        let executor_factory = CustomBlockExecutorFactory::new(inner.executor_factory.clone());
        Ok(CustomEvmConfig { inner, executor_factory })
    }
}

/// Ethereum EVM configuration with a custom block executor.
#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: EthEvmConfig,
    executor_factory: CustomBlockExecutorFactory,
}

impl ConfigureEvm for CustomEvmConfig {
    type Primitives = <EthEvmConfig as ConfigureEvm>::Primitives;
    type Error = <EthEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <EthEvmConfig as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory = CustomBlockExecutorFactory;
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn evm_env(
        &self,
        header: &Header,
    ) -> Result<reth_ethereum::evm::primitives::EvmEnvFor<Self>, Self::Error> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<reth_ethereum::evm::primitives::EvmEnvFor<Self>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<reth_ethereum::evm::primitives::ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a,
    {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<reth_ethereum::evm::primitives::ExecutionCtxFor<'_, Self>, Self::Error> {
        self.inner.context_for_next_block(parent, attributes)
    }

    fn with_jit_support_enabled(self, enabled: bool) -> Self
    where
        Self: Sized,
    {
        let inner = self.inner.with_jit_support_enabled(enabled);
        let executor_factory = CustomBlockExecutorFactory::new(inner.executor_factory.clone());
        Self { inner, executor_factory }
    }

    fn with_precompile_cache_disabled(self, disabled: bool) -> Self
    where
        Self: Sized,
    {
        let inner = self.inner.with_precompile_cache_disabled(disabled);
        let executor_factory = CustomBlockExecutorFactory::new(inner.executor_factory.clone());
        Self { inner, executor_factory }
    }

    fn pre_block_state_changes<'a, DB>(
        &self,
        db: DB,
        evm_env: reth_ethereum::evm::primitives::EvmEnvFor<Self>,
        block_number: u64,
        ctx: reth_ethereum::evm::primitives::ExecutionCtxFor<'a, Self>,
    ) -> Result<reth_ethereum::evm::primitives::EvmState, Box<dyn std::error::Error + Send + Sync>>
    where
        Self: 'a,
        DB: reth_ethereum::evm::primitives::DynDatabase + 'a,
    {
        self.inner.pre_block_state_changes(db, evm_env, block_number, ctx)
    }
}

impl ConfigureEngineEvm<ExecutionData> for CustomEvmConfig {
    fn evm_env_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<reth_ethereum::evm::primitives::EvmEnvFor<Self>, Self::Error> {
        self.inner.evm_env_for_payload(payload)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<reth_ethereum::evm::primitives::ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner.context_for_payload(payload)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let txs = payload.payload.transactions().clone();
        let convert = |tx: Bytes| -> Result<_, std::io::Error> {
            let tx = TransactionSigned::decode_2718_exact(tx.as_ref()).map_err(|err| {
                std::io::Error::other(format!("failed to decode transaction: {err:?}"))
            })?;
            let signer = tx.try_recover().map_err(|err| {
                std::io::Error::other(format!("failed to recover transaction signer: {err:?}"))
            })?;
            Ok(ExecutableRecoveredTx::new(tx.with_signer(signer)))
        };

        Ok((txs, convert))
    }
}

/// Factory that wraps the standard Ethereum executor factory with the custom executor.
#[derive(Debug, Clone)]
pub struct CustomBlockExecutorFactory {
    inner: EthBlockExecutorFactory<RethReceiptBuilder, ChainSpec, RethEvmFactory>,
}

impl CustomBlockExecutorFactory {
    const fn new(
        inner: EthBlockExecutorFactory<RethReceiptBuilder, ChainSpec, RethEvmFactory>,
    ) -> Self {
        Self { inner }
    }
}

impl BlockExecutorFactory for CustomBlockExecutorFactory {
    type EvmFactory = RethEvmFactory;
    type EvmTypes = BaseEvmTypes;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm<'a> = evm2::Evm<'a, BaseEvmTypes>;
    type EvmEnv = EthEvmEnv<BaseEvmTypes>;
    type ExecutionCtx<'a>
        = EthBlockExecutionCtx<'a>
    where
        Self: 'a;
    type Executor<'a>
        = CustomBlockExecutor<'a>
    where
        Self: 'a;

    fn create_executor<'a>(
        &'a self,
        evm: Self::Evm<'a>,
        mut ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a>
    where
        Self: 'a,
    {
        let withdrawals = ctx.withdrawals.take();
        ctx.withdrawals = None;
        let inner = EthBlockExecutor::new(
            evm,
            ctx,
            self.inner.chain_spec().as_ref(),
            self.inner.receipt_builder(),
        );
        CustomBlockExecutor { inner, withdrawals, hashed_state_hook: None }
    }

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn evm_with_env<'a, DB>(&self, db: DB, evm_env: Self::EvmEnv) -> Self::Evm<'a>
    where
        DB: reth_ethereum::evm::primitives::DynDatabase + 'a,
    {
        self.inner.evm_with_env(db, evm_env)
    }
}

/// Block executor that dispatches beacon withdrawals to a contract.
pub struct CustomBlockExecutor<'a> {
    inner: EthBlockExecutor<'a, BaseEvmTypes, &'a RethReceiptBuilder>,
    withdrawals: Option<Cow<'a, [Withdrawal]>>,
    hashed_state_hook: Option<HashedStateHook>,
}

impl<'a> BlockExecutor for CustomBlockExecutor<'a> {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm = evm2::Evm<'a, BaseEvmTypes>;
    type TransactionResultWithState =
        <EthBlockExecutor<'a, BaseEvmTypes, &'a RethReceiptBuilder> as BlockExecutor>::TransactionResultWithState;
    type BlockAccessList =
        <EthBlockExecutor<'a, BaseEvmTypes, &'a RethReceiptBuilder> as BlockExecutor>::BlockAccessList;

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn set_state_hook(&mut self, hook: impl FnMut(HashedPostState) + Send + 'static) -> bool {
        let hook: HashedStateHook = Arc::new(Mutex::new(Box::new(hook)));
        let inner_hook = Arc::clone(&hook);
        if !self.inner.set_state_hook(move |state| {
            let mut hook = inner_hook.lock().expect("hashed state hook mutex poisoned");
            (hook)(state);
        }) {
            return false
        }
        self.hashed_state_hook = Some(hook);
        true
    }

    fn convert_block_access_list(
        block_access_list: &alloy_eips::eip7928::BlockAccessList,
    ) -> Result<Self::BlockAccessList, BlockExecutionError> {
        EthBlockExecutor::<BaseEvmTypes, &'a RethReceiptBuilder>::convert_block_access_list(
            block_access_list,
        )
    }

    fn set_block_access_list(&mut self, block_access_list: Arc<Self::BlockAccessList>) {
        self.inner.set_block_access_list(block_access_list);
    }

    fn set_block_access_index(&mut self, index: alloy_eips::eip7928::BlockAccessIndex) {
        self.inner.set_block_access_index(index);
    }

    fn enable_block_access_list_builder(&mut self) {
        self.inner.enable_block_access_list_builder();
    }

    fn take_block_access_list(&mut self) -> Option<alloy_eips::eip7928::BlockAccessList> {
        self.inner.take_block_access_list()
    }

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_without_commit(
        &mut self,
        transaction: impl ExecutorTx<Self>,
    ) -> Result<Self::TransactionResultWithState, BlockExecutionError> {
        let (tx_env, tx) = transaction.into_parts();
        self.inner.execute_transaction_without_commit((tx_env, tx))
    }

    fn commit_transaction(
        &mut self,
        output: Self::TransactionResultWithState,
    ) -> Result<GasOutput, BlockExecutionError> {
        self.inner.commit_transaction(output)
    }

    fn receipts(&self) -> &[Self::Receipt] {
        self.inner.receipts()
    }

    fn finish_with_block_access_list(
        mut self,
    ) -> Result<
        (BlockExecutionOutput<Self::Receipt>, Option<alloy_eips::eip7928::BlockAccessList>),
        BlockExecutionError,
    > {
        let withdrawal_state = self.apply_withdrawals_contract_call()?;
        if let Some(hook) = &self.hashed_state_hook {
            let hashed_state =
                hashed_post_state_from_execution_state::<KeccakKeyHasher>(&withdrawal_state.inner);
            if !hashed_state.is_empty() {
                let mut hook = hook.lock().expect("hashed state hook mutex poisoned");
                (hook)(hashed_state);
            }
        }
        let (mut output, block_access_list) = self.inner.finish_with_block_access_list()?;

        let mut state = output.state.into_inner();
        withdrawal_state.inner.visit(&mut state).expect("withdrawal state sink is infallible");
        output.state = state.into();

        Ok((output, block_access_list))
    }
}

impl CustomBlockExecutor<'_> {
    fn apply_withdrawals_contract_call(&mut self) -> Result<WithdrawalState, BlockExecutionError> {
        let beneficiary = self.inner.evm().block().beneficiary;
        let mut state = WithdrawalState { inner: BlockStateAccumulator::new(), beneficiary };
        let Some(withdrawals) = self.withdrawals.as_deref() else {
            return Ok(state);
        };

        let calldata = withdrawalsCall {
            amounts: withdrawals.iter().map(|withdrawal| withdrawal.amount).collect(),
            addresses: withdrawals.iter().map(|withdrawal| withdrawal.address).collect(),
        }
        .abi_encode()
        .into();

        let executed = self
            .inner
            .evm_mut()
            .system_call(SystemTx::new(WITHDRAWALS_ADDRESS, calldata))
            .map_err(|err| {
                BlockExecutionError::msg(format!("withdrawal contract system call failed: {err:?}"))
            })?;

        if !executed.result().status {
            let reason = format!("{:?}", executed.result().stop);
            let _ = executed.discard();
            return Err(BlockExecutionError::msg(format!(
                "withdrawal contract system call reverted: {reason}"
            )));
        }

        let _ = executed.commit_with(&mut state).expect("withdrawal state sink is infallible");
        Ok(state)
    }
}

#[derive(Debug)]
struct WithdrawalState {
    inner: BlockStateAccumulator,
    beneficiary: Address,
}

impl StateChangeSink for WithdrawalState {
    type Error = Infallible;

    fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        if change.address == SYSTEM_ADDRESS || change.address == self.beneficiary {
            return Ok(())
        }
        StateChangeSink::account(&mut self.inner, change)
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        if address == SYSTEM_ADDRESS || address == self.beneficiary {
            return Ok(())
        }
        StateChangeSink::storage_wipe(&mut self.inner, address)
    }

    fn storage(&mut self, change: StorageChange) -> Result<(), Self::Error> {
        if change.address == SYSTEM_ADDRESS || change.address == self.beneficiary {
            return Ok(())
        }
        StateChangeSink::storage(&mut self.inner, change)
    }
}

sol! {
    function withdrawals(uint64[] calldata amounts, address[] calldata addresses);
}
