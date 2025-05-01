use alloy_eips::eip7685::Requests;
use alloy_evm::Evm;
use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_chainspec::ChainSpec;
use reth_ethereum_primitives::{BlockBody, Receipt, TransactionSigned};
use reth_evm::{
    block::{BlockExecutor, BlockExecutorFactory},
    eth::{EthBlockExecutionCtx, EthEvmContext},
    ConfigureEvm, Database, EthEvm, EthEvmFactory, EvmFactory,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_network_p2p::test_utils::TestFullBlockClient;
use reth_primitives_traits::SealedHeader;
use reth_provider::{
    test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
    BlockExecutionResult, ExecutionOutcome,
};
use reth_prune_types::PruneModes;
use reth_revm::{Inspector, State};
use reth_stages::{test_utils::TestStages, ExecOutput, StageError};
use reth_stages_api::Pipeline;
use reth_static_file::StaticFileProducer;
use std::{collections::VecDeque, ops::Range, sync::Arc};
use tokio::sync::watch;

/// Test pipeline builder.
#[derive(Default, Debug)]
pub struct TestPipelineBuilder {
    pipeline_exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    executor_results: Vec<ExecutionOutcome>,
}

impl TestPipelineBuilder {
    /// Create a new [`TestPipelineBuilder`].
    pub const fn new() -> Self {
        Self { pipeline_exec_outputs: VecDeque::new(), executor_results: Vec::new() }
    }

    /// Set the pipeline execution outputs to use for the test consensus engine.
    pub fn with_pipeline_exec_outputs(
        mut self,
        pipeline_exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    ) -> Self {
        self.pipeline_exec_outputs = pipeline_exec_outputs;
        self
    }

    /// Set the executor results to use for the test consensus engine.
    pub fn with_executor_results(mut self, executor_results: Vec<ExecutionOutcome>) -> Self {
        self.executor_results = executor_results;
        self
    }

    /// Builds the pipeline.
    pub fn build(self, chain_spec: Arc<ChainSpec>) -> Pipeline<MockNodeTypesWithDB> {
        reth_tracing::init_test_tracing();

        // Setup pipeline
        let (tip_tx, _tip_rx) = watch::channel(B256::default());
        let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stages(TestStages::new(self.pipeline_exec_outputs, Default::default()))
            .with_tip_sender(tip_tx);

        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec);

        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

        pipeline.build(provider_factory, static_file_producer)
    }
}

/// Starting from the given genesis header, inserts headers from the given
/// range in the given test full block client.
pub fn insert_headers_into_client(
    client: &TestFullBlockClient,
    genesis_header: SealedHeader,
    range: Range<usize>,
) {
    let mut sealed_header = genesis_header;
    let body = BlockBody::default();
    for _ in range {
        let (mut header, hash) = sealed_header.split();
        // update to the next header
        header.parent_hash = hash;
        header.number += 1;
        header.timestamp += 1;
        sealed_header = SealedHeader::seal_slow(header);
        client.insert(sealed_header.clone(), body.clone());
    }
}

/// A block executor provider that returns mocked execution results.
#[derive(Clone, Debug)]
pub struct MockEvmConfig {
    inner: EthEvmConfig,
    exec_results: Arc<Mutex<Vec<ExecutionOutcome>>>,
}

impl MockEvmConfig {
    /// Create a new [`MockEvmConfig`].
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        let inner = EthEvmConfig::new(chain_spec);
        Self { inner, exec_results: Default::default() }
    }

    /// Extend the mocked execution results
    pub fn extend(&self, results: impl IntoIterator<Item = impl Into<ExecutionOutcome>>) {
        self.exec_results.lock().extend(results.into_iter().map(Into::into));
    }
}

impl BlockExecutorFactory for MockEvmConfig {
    type EvmFactory = EthEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Receipt = Receipt;
    type Transaction = reth_ethereum_primitives::TransactionSigned;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EthEvm<&'a mut State<DB>, I>,
        _ctx: Self::ExecutionCtx<'a>,
    ) -> impl alloy_evm::block::BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: Inspector<<Self::EvmFactory as EvmFactory>::Context<&'a mut State<DB>>> + 'a,
    {
        MockExecutor { result: self.exec_results.lock().pop().unwrap(), evm, hook: None }
    }
}

/// Mock executor that returns a fixed execution result.
#[derive(derive_more::Debug)]
pub struct MockExecutor<'a, DB: Database, I> {
    result: ExecutionOutcome,
    evm: EthEvm<&'a mut State<DB>, I>,
    #[debug(skip)]
    hook: Option<Box<dyn reth_evm::OnStateHook>>,
}

impl<'a, DB: Database, I: Inspector<EthEvmContext<&'a mut State<DB>>>> BlockExecutor
    for MockExecutor<'a, DB, I>
{
    type Evm = EthEvm<&'a mut State<DB>, I>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn apply_pre_execution_changes(&mut self) -> Result<(), reth_errors::BlockExecutionError> {
        Ok(())
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        _tx: impl reth_evm::block::ExecutableTx<Self>,
        _f: impl FnOnce(
            &reth_revm::context::result::ExecutionResult<<Self::Evm as reth_evm::Evm>::HaltReason>,
        ),
    ) -> Result<u64, reth_errors::BlockExecutionError> {
        Ok(0)
    }

    fn finish(
        self,
    ) -> Result<
        (Self::Evm, reth_provider::BlockExecutionResult<Self::Receipt>),
        reth_errors::BlockExecutionError,
    > {
        let Self { result, mut evm, .. } = self;
        let ExecutionOutcome { bundle, receipts, requests, first_block: _ } = result;
        let result = BlockExecutionResult {
            receipts: receipts.into_iter().flatten().collect(),
            requests: requests.into_iter().fold(Requests::default(), |mut reqs, req| {
                reqs.extend(req);
                reqs
            }),
            gas_used: 0,
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

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
    }

    fn builder_for_next_block<'a, DB: Database>(
        &'a self,
        db: &'a mut reth_revm::State<DB>,
        parent: &'a SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<impl reth_evm::execute::BlockBuilder<Primitives = Self::Primitives>, Self::Error>
    {
        self.inner.builder_for_next_block(db, parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a reth_primitives_traits::SealedBlock<
            reth_primitives_traits::BlockTy<Self::Primitives>,
        >,
    ) -> reth_evm::ExecutionCtxFor<'a, Self> {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<reth_primitives_traits::HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> reth_evm::ExecutionCtxFor<'_, Self> {
        self.inner.context_for_next_block(parent, attributes)
    }

    fn create_block_builder<'a, DB, I>(
        &'a self,
        evm: reth_evm::EvmFor<Self, &'a mut reth_revm::State<DB>, I>,
        parent: &'a SealedHeader<reth_primitives_traits::HeaderTy<Self::Primitives>>,
        ctx: <Self::BlockExecutorFactory as reth_evm::block::BlockExecutorFactory>::ExecutionCtx<
            'a,
        >,
    ) -> impl reth_evm::execute::BlockBuilder<
        Primitives = Self::Primitives,
        Executor: reth_evm::block::BlockExecutorFor<'a, Self::BlockExecutorFactory, DB, I>,
    >
    where
        DB: reth_evm::Database,
        I: reth_evm::InspectorFor<Self, &'a mut reth_revm::State<DB>> + 'a,
    {
        self.inner.create_block_builder(evm, parent, ctx)
    }

    fn evm_env(
        &self,
        header: &reth_primitives_traits::HeaderTy<Self::Primitives>,
    ) -> reth_evm::EvmEnvFor<Self> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &reth_primitives_traits::HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<reth_evm::EvmEnvFor<Self>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }
}
