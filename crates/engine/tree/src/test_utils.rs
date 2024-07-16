use crate::tree::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_db::{mdbx::DatabaseEnv, test_utils::TempDatabase};
use reth_network_p2p::test_utils::TestFullBlockClient;
use reth_primitives::{
    Address, Block, BlockBody, BlockNumber, Receipts, Requests, SealedBlockWithSenders,
    SealedHeader, TransactionSigned, B256,
};
use reth_provider::{test_utils::create_test_provider_factory_with_chain_spec, ExecutionOutcome};
use reth_prune_types::PruneModes;
use reth_stages::{test_utils::TestStages, ExecOutput, StageError};
use reth_stages_api::Pipeline;
use reth_static_file::StaticFileProducer;
use reth_trie::{updates::TrieUpdates, HashedPostState};
use revm::db::BundleState;
use std::{collections::VecDeque, ops::Range, sync::Arc};
use tokio::sync::watch;

/// Test pipeline builder.
#[derive(Default)]
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
    #[allow(dead_code)]
    pub fn with_executor_results(mut self, executor_results: Vec<ExecutionOutcome>) -> Self {
        self.executor_results = executor_results;
        self
    }

    /// Builds the pipeline.
    pub fn build(self, chain_spec: Arc<ChainSpec>) -> Pipeline<Arc<TempDatabase<DatabaseEnv>>> {
        reth_tracing::init_test_tracing();

        // Setup pipeline
        let (tip_tx, _tip_rx) = watch::channel(B256::default());
        let pipeline = Pipeline::builder()
            .add_stages(TestStages::new(self.pipeline_exec_outputs, Default::default()))
            .with_tip_sender(tip_tx);

        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec);

        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

        pipeline.build(provider_factory, static_file_producer)
    }
}

pub(crate) fn insert_headers_into_client(
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
        sealed_header = header.seal_slow();
        client.insert(sealed_header.clone(), body.clone());
    }
}

pub(crate) fn get_executed_block_with_number(block_number: BlockNumber) -> ExecutedBlock {
    let mut block = Block::default();
    let mut header = block.header.clone();
    header.number = block_number;
    block.header = header;

    let sender = Address::random();
    let tx = TransactionSigned::default();
    block.body.push(tx);
    let sealed = block.seal_slow();
    let sealed_with_senders = SealedBlockWithSenders::new(sealed.clone(), vec![sender]).unwrap();

    ExecutedBlock::new(
        Arc::new(sealed),
        Arc::new(sealed_with_senders.senders),
        Arc::new(ExecutionOutcome::new(
            BundleState::default(),
            Receipts { receipt_vec: vec![vec![]] },
            block_number,
            vec![Requests::default()],
        )),
        Arc::new(HashedPostState::default()),
        Arc::new(TrieUpdates::default()),
    )
}

pub(crate) fn get_executed_blocks(range: Range<u64>) -> impl Iterator<Item = ExecutedBlock> {
    range.map(get_executed_block_with_number)
}
