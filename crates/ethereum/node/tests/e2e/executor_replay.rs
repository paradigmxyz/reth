//! Replays of a block's transactions run through the chain's block executor.

use alloy_consensus::{Header, Transaction};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Address, B256, U256};
use alloy_provider::{ext::DebugApi, Provider};
use alloy_rpc_types_engine::ExecutionData;
use alloy_rpc_types_eth::TransactionReceipt;
use alloy_rpc_types_trace::{
    geth::{
        DiffMode, GethDebugTracingOptions, GethTrace, PreStateConfig, PreStateFrame, TraceResult,
    },
    parity::{Delta, TraceResultsWithTransactionHash},
};
use evm2::BaseEvmTypes;
use jsonrpsee::core::client::ClientT;
use reth_chainspec::{ChainSpec, EthereumHardfork};
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt, TransactionSigned};
use reth_evm::{
    precompile_cache::PrecompileCacheMap, BlockExecutionError, BlockExecutionOutput, BlockExecutor,
    BlockExecutorFactory, BundleState, ConfigureEngineEvm, ConfigureEvm, DynDatabase, EvmEnvFor,
    EvmFor, ExecutableTxIterator, ExecutionCtxFor, ExecutorTx, GasOutput, NextBlockEnvAttributes,
    RecoveredTx, SpecFor, StateUpdate,
};
use reth_evm_ethereum::{
    EthBlockAssembler, EthBlockExecutionCtx, EthBlockExecutor, EthBlockExecutorFactory,
    EthEvmConfig, EthEvmEnv, EthTransactionResultWithState, RethEvmFactory, RethReceiptBuilder,
};
use reth_node_builder::{
    components::{BasicPayloadServiceBuilder, ComponentsBuilder, ExecutorBuilder},
    node::{FullNodeTypes, NodeTypes},
    BuilderContext, Node, NodeAdapter,
};
use reth_node_ethereum::{
    EthEngineTypes, EthereumAddOns, EthereumConsensusBuilder, EthereumEngineValidatorBuilder,
    EthereumEthApiBuilder, EthereumNetworkBuilder, EthereumNode, EthereumPayloadBuilder,
    EthereumPoolBuilder,
};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use reth_provider::EthStorage;
use std::sync::Arc;

/// Amount the test executor credits to the sender of every transaction.
const SETTLEMENT: U256 = U256::from_limbs([1_000_000_000, 0, 0, 0]);

/// Traces and intermediate roots of a block's transactions include the changes the block executor
/// applies on top of the EVM result, so they agree with the committed state, both when the replay
/// executes the transactions before the target and when it positions the state with the block's
/// BAL.
#[tokio::test]
async fn replays_include_block_executor_settlement() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, wallet) = SettlingNode::test_setup_for(EthereumHardfork::Prague)
        .with_node_config_modifier(|mut config| {
            config.rpc.rpc_state_cache.cache_computed_bals = true;
            config
        })
        .build_single()
        .await?;
    let provider = node.rpc_provider();
    let client = node.rpc_client().expect("the HTTP RPC server is enabled");
    let mut account = wallet.account(0);
    let sender = account.address();
    let recipient = Address::with_last_byte(0x77);
    let values = [U256::from(1), U256::from(2)];

    let first = account.transfer(recipient, values[0]).await;
    let second = account.transfer(recipient, values[1]).await;
    let mined = node.mine([first, second]).await?.ensure_success()?;
    let block_hash = mined.block().hash();
    let state_root = mined.block().state_root;

    // The executor's settlement is part of the committed state.
    let mut balance = provider.get_balance(sender).number(0).await?;
    let mut expected = Vec::new();
    for (receipt, value) in mined.receipts.iter().zip(values) {
        let pre = balance;
        balance = pre - value - fee(receipt) + SETTLEMENT;
        expected.push((receipt.transaction_hash, pre, balance));
    }
    assert_eq!(provider.get_balance(sender).number(1).await?, balance);

    let opts = GethDebugTracingOptions::prestate_tracer(PreStateConfig {
        diff_mode: Some(true),
        ..Default::default()
    });
    let trace_transactions = async || -> eyre::Result<Vec<DiffMode>> {
        let mut diffs = Vec::new();
        for &(tx_hash, pre, post) in &expected {
            let diff =
                prestate_diff(provider.debug_trace_transaction(tx_hash, opts.clone()).await?)?;
            assert_eq!(diff.pre[&sender].balance, Some(pre));
            assert_eq!(diff.post[&sender].balance, Some(post));
            diffs.push(diff);
        }
        Ok(diffs)
    };

    // Without a cached BAL, the transactions before the target are replayed.
    let cache = node.rpc.inner.eth_api().cache();
    assert!(cache.get_recovered_block_and_maybe_bal(block_hash).await?.unwrap().1.is_none());
    let diffs = trace_transactions().await?;

    let block_diffs = provider
        .debug_trace_block_by_number(BlockNumberOrTag::Number(1), opts.clone())
        .await?
        .into_iter()
        .map(|result| match result {
            TraceResult::Success { result, .. } => prestate_diff(result),
            TraceResult::Error { error, .. } => Err(eyre::eyre!("trace failed: {error}")),
        })
        .collect::<eyre::Result<Vec<_>>>()?;
    assert_eq!(block_diffs, diffs);

    let replays: Vec<TraceResultsWithTransactionHash> = client
        .request("trace_replayBlockTransactions", (BlockNumberOrTag::Number(1), ["stateDiff"]))
        .await?;
    assert_eq!(replays.len(), expected.len());
    for (replay, &(tx_hash, pre, post)) in replays.iter().zip(&expected) {
        assert_eq!(replay.transaction_hash, tx_hash);
        let state_diff = replay.full_trace.state_diff.as_ref().expect("stateDiff was requested");
        assert_eq!(state_diff.0[&sender].balance, Delta::changed(pre, post));
    }

    let roots =
        client.request::<Vec<B256>, _>("debug_intermediateRoots", (block_hash, None::<()>)).await?;
    assert_eq!(roots.len(), 2);
    assert_eq!(roots.last(), Some(&state_root));

    // With the block's BAL cached, only the target transaction is executed.
    let _: serde_json::Value = client.request("eth_getBlockAccessList", (block_hash,)).await?;
    assert!(cache.get_recovered_block_and_maybe_bal(block_hash).await?.unwrap().1.is_some());
    assert_eq!(trace_transactions().await?, diffs);

    Ok(())
}

fn fee(receipt: &TransactionReceipt) -> U256 {
    U256::from(receipt.gas_used) * U256::from(receipt.effective_gas_price)
}

fn prestate_diff(trace: GethTrace) -> eyre::Result<DiffMode> {
    match trace {
        GethTrace::PreStateTracer(PreStateFrame::Diff(diff)) => Ok(diff),
        trace => Err(eyre::eyre!("expected a prestate diff, got {trace:?}")),
    }
}

/// Ethereum node whose block executor credits `SETTLEMENT` to every transaction's sender.
#[derive(Debug, Default, Clone, Copy)]
struct SettlingNode;

impl NodeTypes for SettlingNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = EthStorage;
    type Payload = EthEngineTypes;
}

impl<N> Node<N> for SettlingNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        EthereumNetworkBuilder,
        SettlingExecutorBuilder,
        EthereumConsensusBuilder,
    >;

    type AddOns =
        EthereumAddOns<NodeAdapter<N>, EthereumEthApiBuilder, EthereumEngineValidatorBuilder>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        EthereumNode::components().executor(SettlingExecutorBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        EthereumAddOns::default()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct SettlingExecutorBuilder;

impl<N> ExecutorBuilder<N> for SettlingExecutorBuilder
where
    N: FullNodeTypes<Types = SettlingNode>,
{
    type EVM = SettlingEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::EVM> {
        let inner = EthEvmConfig::new(ctx.chain_spec());
        let executor_factory = SettlingExecutorFactory { inner: inner.executor_factory.clone() };
        Ok(SettlingEvmConfig { inner, executor_factory })
    }
}

#[derive(Debug, Clone)]
struct SettlingEvmConfig {
    inner: EthEvmConfig,
    executor_factory: SettlingExecutorFactory,
}

impl ConfigureEvm for SettlingEvmConfig {
    type Primitives = EthPrimitives;
    type Error = <EthEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = SettlingExecutorFactory;
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a,
    {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'_, Self>, Self::Error> {
        self.inner.context_for_next_block(parent, attributes)
    }

    fn evm_with_precompile_cache<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        cache_map: PrecompileCacheMap<SpecFor<Self>>,
        metrics_enabled: bool,
    ) -> EvmFor<'a, Self>
    where
        DB: evm2::evm::Database + 'a,
    {
        self.inner.evm_with_precompile_cache(db, evm_env, cache_map, metrics_enabled)
    }
}

impl ConfigureEngineEvm<ExecutionData> for SettlingEvmConfig {
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.evm_env_for_payload(payload)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner.context_for_payload(payload)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        self.inner.tx_iterator_for_payload(payload)
    }
}

#[derive(Debug, Clone)]
struct SettlingExecutorFactory {
    inner: EthBlockExecutorFactory<RethReceiptBuilder, ChainSpec, RethEvmFactory>,
}

impl BlockExecutorFactory for SettlingExecutorFactory {
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
        = SettlingExecutor<'a>
    where
        Self: 'a;

    fn create_executor<'a>(
        &'a self,
        evm: Self::Evm<'a>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a>
    where
        Self: 'a,
    {
        SettlingExecutor {
            inner: EthBlockExecutor::new(
                evm,
                ctx,
                self.inner.chain_spec().as_ref(),
                self.inner.receipt_builder(),
            ),
        }
    }

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn evm_with_env<'a, DB>(&self, db: DB, evm_env: Self::EvmEnv) -> Self::Evm<'a>
    where
        DB: DynDatabase + 'a,
    {
        self.inner.evm_with_env(db, evm_env)
    }
}

type InnerExecutor<'a> = EthBlockExecutor<'a, BaseEvmTypes, &'a RethReceiptBuilder>;

/// Ethereum block executor that credits `SETTLEMENT` to the sender in each transaction's detached
/// state, like a chain settling a per-transaction refund outside the EVM.
struct SettlingExecutor<'a> {
    inner: InnerExecutor<'a>,
}

impl<'a> BlockExecutor for SettlingExecutor<'a> {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm = evm2::Evm<'a, BaseEvmTypes>;
    type TransactionResultWithState = EthTransactionResultWithState;
    type BlockAccessList = <InnerExecutor<'a> as BlockExecutor>::BlockAccessList;

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn set_state_hook(&mut self, hook: impl FnMut(StateUpdate) + Send + 'static) -> bool {
        self.inner.set_state_hook(hook)
    }

    fn convert_block_access_list(
        block_access_list: &alloy_eips::eip7928::BlockAccessList,
    ) -> Result<Self::BlockAccessList, BlockExecutionError> {
        InnerExecutor::convert_block_access_list(block_access_list)
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

    fn into_state(self) -> BundleState {
        self.inner.into_state()
    }

    fn validate_transaction_gas_limit(
        &mut self,
        gas_limit: u64,
    ) -> Result<(), BlockExecutionError> {
        self.inner.validate_transaction_gas_limit(gas_limit)
    }

    fn execute_transaction_without_commit(
        &mut self,
        transaction: impl ExecutorTx<Self>,
    ) -> Result<Self::TransactionResultWithState, BlockExecutionError> {
        let (tx_env, tx) = transaction.into_parts();
        let sender = *tx.signer();
        let tx_type = tx.tx().tx_type();
        let blob_gas_used = tx.tx().blob_gas_used().unwrap_or_default();

        let (mut result, ..) =
            self.inner.execute_transaction_without_commit((tx_env, tx))?.into_parts();
        let account = result
            .pending_state
            .account_changes_for(&sender)
            .expect("every transaction loads its sender");
        let original = account.original.cloned();
        let mut settled = account.current.cloned().expect("the sender exists after paying fees");
        settled.balance += SETTLEMENT;
        result.pending_state.insert_account(sender, original, Some(settled));

        Ok(EthTransactionResultWithState::new(result, tx_type, blob_gas_used))
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
        self,
    ) -> Result<
        (BlockExecutionOutput<Self::Receipt>, Option<alloy_eips::eip7928::BlockAccessList>),
        BlockExecutionError,
    > {
        self.inner.finish_with_block_access_list()
    }
}
