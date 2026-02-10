use crate::EthEvmConfig;
use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use alloy_consensus::{Header, TxType};
use alloy_eips::eip7685::Requests;
use alloy_evm::precompiles::PrecompilesMap;
use alloy_primitives::Bytes;
use alloy_rpc_types_engine::ExecutionData;
use parking_lot::Mutex;
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::{
    block::{
        BlockExecutionError, BlockExecutor, BlockExecutorFactory, BlockExecutorFor, ExecutableTx,
    },
    eth::{EthBlockExecutionCtx, EthEvmContext, EthTxResult},
    ConfigureEngineEvm, ConfigureEvm, Database, EthEvm, EthEvmFactory, Evm, EvmEnvFor, EvmFactory,
    ExecutableTxIterator, ExecutionCtxFor, RecoveredTx,
};
use reth_execution_types::{BlockExecutionResult, ExecutionOutcome};
use reth_primitives_traits::{BlockTy, SealedBlock, SealedHeader};
use revm::{
    context::result::{ExecutionResult, HaltReason, Output, ResultAndState, SuccessReason},
    database::State,
    Inspector,
};

/// A helper type alias for mocked block executor provider.
pub type MockExecutorProvider = MockEvmConfig;

/// A block executor provider that returns mocked execution results.
#[derive(Clone, Debug)]
pub struct MockEvmConfig {
    inner: EthEvmConfig,
    exec_results: Arc<Mutex<Vec<ExecutionOutcome>>>,
}

impl Default for MockEvmConfig {
    fn default() -> Self {
        Self { inner: EthEvmConfig::mainnet(), exec_results: Default::default() }
    }
}

impl MockEvmConfig {
    /// Extend the mocked execution results
    pub fn extend(&self, results: impl IntoIterator<Item = impl Into<ExecutionOutcome>>) {
        self.exec_results.lock().extend(results.into_iter().map(Into::into));
    }
}

impl BlockExecutorFactory for MockEvmConfig {
    type EvmFactory = EthEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Receipt = Receipt;
    type Transaction = TransactionSigned;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EthEvm<&'a mut State<DB>, I, PrecompilesMap>,
        _ctx: Self::ExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: Inspector<<Self::EvmFactory as EvmFactory>::Context<&'a mut State<DB>>> + 'a,
    {
        MockExecutor {
            result: self.exec_results.lock().pop().unwrap(),
            evm,
            hook: None,
            receipts: Vec::new(),
        }
    }
}

/// Mock executor that returns a fixed execution result.
#[derive(derive_more::Debug)]
pub struct MockExecutor<'a, DB: Database, I> {
    result: ExecutionOutcome,
    evm: EthEvm<&'a mut State<DB>, I, PrecompilesMap>,
    #[debug(skip)]
    hook: Option<Box<dyn reth_evm::OnStateHook>>,
    receipts: Vec<Receipt>,
}

impl<'a, DB: Database, I: Inspector<EthEvmContext<&'a mut State<DB>>>> BlockExecutor
    for MockExecutor<'a, DB, I>
{
    type Evm = EthEvm<&'a mut State<DB>, I, PrecompilesMap>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Result = EthTxResult<HaltReason, TxType>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        Ok(())
    }

    fn receipts(&self) -> &[Self::Receipt] {
        &self.receipts
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        Ok(EthTxResult {
            result: ResultAndState::new(
                ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas: Default::default(),
                    logs: vec![],
                    output: Output::Call(Bytes::from(vec![])),
                },
                Default::default(),
            ),
            tx_type: tx.into_parts().1.tx().tx_type(),
            blob_gas_used: 0,
        })
    }

    fn commit_transaction(&mut self, _output: Self::Result) -> Result<u64, BlockExecutionError> {
        Ok(0)
    }

    fn finish(
        self,
    ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
        let Self { result, mut evm, .. } = self;
        let ExecutionOutcome { bundle, receipts, requests, first_block: _ } = result;
        let result = BlockExecutionResult {
            receipts: receipts.into_iter().flatten().collect(),
            requests: requests.into_iter().fold(Requests::default(), |mut reqs, req| {
                reqs.extend(req);
                reqs
            }),
            gas_used: 0,
            blob_gas_used: 0,
        };

        evm.db_mut().bundle_state = bundle;

        Ok((evm, result))
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn reth_evm::OnStateHook>>) {
        self.hook = hook;
    }

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }
}

impl ConfigureEvm for MockEvmConfig {
    type BlockAssembler = <EthEvmConfig as ConfigureEvm>::BlockAssembler;
    type BlockExecutorFactory = Self;
    type Error = <EthEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <EthEvmConfig as ConfigureEvm>::NextBlockEnvCtx;
    type Primitives = <EthEvmConfig as ConfigureEvm>::Primitives;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
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
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<reth_evm::ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<reth_evm::ExecutionCtxFor<'_, Self>, Self::Error> {
        self.inner.context_for_next_block(parent, attributes)
    }
}

impl ConfigureEngineEvm<ExecutionData> for MockEvmConfig {
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
