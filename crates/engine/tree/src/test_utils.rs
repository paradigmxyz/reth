use alloy_primitives::{Sealable, B256};
use reth_chainspec::ChainSpec;
use reth_network_p2p::test_utils::TestFullBlockClient;
use reth_primitives::{BlockBody, SealedHeader};
use reth_provider::{
    test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
    ExecutionOutcome,
};
use reth_prune_types::PruneModes;
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
    #[allow(dead_code)]
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
        let sealed = header.seal_slow();
        let (header, seal) = sealed.into_parts();
        sealed_header = SealedHeader::new(header, seal);
        client.insert(sealed_header.clone(), body.clone());
    }
}
