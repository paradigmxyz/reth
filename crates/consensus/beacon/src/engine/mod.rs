use futures::{Future, FutureExt, StreamExt};
use reth_db::{database::Database, tables, transaction::DbTx};
use reth_executor::blockchain_tree::{BlockStatus, BlockchainTree};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    executor::Error as ExecutorError,
    sync::SyncStateUpdater,
    Error,
};
use reth_primitives::{BlockHash, SealedBlock, H256};
use reth_provider::ExecutorFactory;
use reth_rpc_types::engine::{
    ForkchoiceUpdated, PayloadAttributes, PayloadStatus, PayloadStatusEnum,
};
use reth_stages::Pipeline;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

mod error;
pub use error::{BeaconEngineError, BeaconEngineResult};

mod message;
pub use message::{BeaconEngineMessage, BeaconEngineSender};

mod pipeline_state;
pub use pipeline_state::PipelineState;

#[derive(Debug, Default)]
enum BeaconEngineAction {
    #[default]
    None,
    RunPipeline,
}

impl BeaconEngineAction {
    fn run_pipeline(&self) -> bool {
        matches!(self, BeaconEngineAction::RunPipeline)
    }
}

/// The beacon consensus engine is the driver that switches between historical and live sync.
///
/// TODO: add more docs
#[must_use = "Future does nothing unless polled"]
pub struct BeaconConsensusEngine<
    DB: Database,
    U: SyncStateUpdater,
    C: Consensus,
    EF: ExecutorFactory,
> {
    db: Arc<DB>,
    pipeline_state: Option<PipelineState<DB, U>>,
    blockchain_tree: BlockchainTree<DB, C, EF>,
    message_rx: UnboundedReceiverStream<BeaconEngineMessage>,
    forkchoice_state: Option<ForkchoiceState>,
    next_action: BeaconEngineAction,
}

impl<DB, U, C, EF> BeaconConsensusEngine<DB, U, C, EF>
where
    DB: Database + Unpin + 'static,
    U: SyncStateUpdater + Unpin + 'static,
    C: Consensus,
    EF: ExecutorFactory + 'static,
{
    /// Create new instance of the [BeaconConsensusEngine].
    pub fn new(
        db: Arc<DB>,
        pipeline: Pipeline<DB, U>,
        blockchain_tree: BlockchainTree<DB, C, EF>,
        message_rx: UnboundedReceiver<BeaconEngineMessage>,
    ) -> Self {
        Self {
            db,
            pipeline_state: Some(PipelineState::Idle(pipeline)),
            blockchain_tree,
            message_rx: UnboundedReceiverStream::new(message_rx),
            forkchoice_state: None,
            next_action: BeaconEngineAction::RunPipeline,
        }
    }

    /// Returns `true` if the pipeline is currently idle.
    fn pipeline_is_idle(&self) -> bool {
        self.pipeline_state.as_ref().expect("pipeline state is set").is_idle()
    }

    /// Set next action to [SyncControllerAction::RunPipeline] to indicate that
    /// controller needs to run the pipeline as soon as it becomes available.
    fn pipeline_run_needed(&mut self) {
        self.next_action = BeaconEngineAction::RunPipeline;
    }

    /// Called to resolve chain forks and ensure that the Execution layer is working with the latest
    /// valid chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        _attrs: Option<PayloadAttributes>,
    ) -> ForkchoiceUpdated {
        self.forkchoice_state = Some(state.clone());
        let status = if self.pipeline_is_idle() {
            match self.blockchain_tree.make_canonical(&state.head_block_hash) {
                Ok(_) => PayloadStatusEnum::Valid,
                Err(error) => {
                    // TODO: match error? differentiate between syncing and invalid
                    error!(target: "consensus::engine", ?state, ?error, "Error canonicalizing the head hash");
                    self.pipeline_run_needed();
                    PayloadStatusEnum::Syncing
                }
            }
        } else {
            PayloadStatusEnum::Syncing
        };
        ForkchoiceUpdated::new(PayloadStatus::from_status(status))
    }

    /// When the Consensus layer receives a new block via the consensus gossip protocol,
    /// the transactions in the block are sent to the execution layer in the form of a
    /// `ExecutionPayload`. The Execution layer executes the transactions and validates the
    /// state in the block header, then passes validation data back to Consensus layer, that
    /// adds the block to the head of its own blockchain and attests to it. The block is then
    /// broadcasted over the consensus p2p network in the form of a "Beacon block".
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_newPayload`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification).
    fn on_new_payload(&mut self, block: SealedBlock) -> PayloadStatus {
        if self.pipeline_is_idle() {
            let block_hash = block.hash;
            match self.blockchain_tree.insert_block(block) {
                Ok(status) => {
                    let latest_valid_hash =
                        matches!(status, BlockStatus::Valid).then_some(block_hash);
                    let status = match status {
                        BlockStatus::Valid => PayloadStatusEnum::Valid,
                        BlockStatus::Accepted => PayloadStatusEnum::Accepted,
                        BlockStatus::Disconnected => PayloadStatusEnum::Syncing,
                    };
                    PayloadStatus::new(status, latest_valid_hash)
                }
                Err(error) => {
                    let latest_valid_hash =
                        matches!(error, Error::Execution(ExecutorError::BlockPreMerge))
                            .then_some(H256::zero());
                    PayloadStatus::new(
                        PayloadStatusEnum::Invalid { validation_error: error.to_string() },
                        latest_valid_hash,
                    )
                }
            }
        } else {
            PayloadStatus::from_status(PayloadStatusEnum::Syncing)
        }
    }

    /// Returns the next pipeline state depending on the current value of the next action.
    /// Resets the next action to the default value.
    fn next_pipeline_state(
        &mut self,
        pipeline: Pipeline<DB, U>,
        tip: H256,
    ) -> PipelineState<DB, U> {
        let next_action = std::mem::take(&mut self.next_action);
        if next_action.run_pipeline() {
            trace!(target: "consensus::engine", ?tip, "Starting the pipeline");
            PipelineState::Running(pipeline.run_as_fut(self.db.clone(), tip))
        } else {
            PipelineState::Idle(pipeline)
        }
    }

    /// Attempt to restore the tree with the finalized block number.
    /// If the finalized block is missing from the database, trigger the pipeline run.
    fn restore_tree_if_possible(
        &mut self,
        finalized_hash: BlockHash,
    ) -> Result<(), BeaconEngineError> {
        match self.db.view(|tx| tx.get::<tables::HeaderNumbers>(finalized_hash))?? {
            Some(number) => self.blockchain_tree.restore_canonical_hashes(number)?,
            None => self.pipeline_run_needed(),
        };
        Ok(())
    }
}

/// On initialization, the controller will poll the message receiver and return [Poll::Pending]
/// until the first forkchoice update message is received.
///
/// As soon as the controller receives the first forkchoice updated message and updates the local
/// forkchoice state, it will launch the pipeline to sync to the head hash.
/// While the pipeline is syncing, the controller will keep processing messages from the receiver
/// and forwarding them to the blockchain tree.
impl<DB, U, C, EF> Future for BeaconConsensusEngine<DB, U, C, EF>
where
    DB: Database + Unpin + 'static,
    U: SyncStateUpdater + Unpin + 'static,
    C: Consensus + Unpin,
    EF: ExecutorFactory + Unpin + 'static,
{
    type Output = Result<(), BeaconEngineError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process all incoming messages first.
        while let Poll::Ready(Some(msg)) = this.message_rx.poll_next_unpin(cx) {
            match msg {
                BeaconEngineMessage::ForkchoiceUpdated(state, attrs, tx) => {
                    let response = this.on_forkchoice_updated(state, attrs);
                    let _ = tx.send(Ok(response));
                }
                BeaconEngineMessage::NewPayload(block, tx) => {
                    let response = this.on_new_payload(block);
                    let _ = tx.send(Ok(response));
                }
            }
        }

        // Set the next pipeline state.
        loop {
            // Lookup the forkchoice state. We can't launch the pipeline without the tip.
            let forckchoice_state = match &this.forkchoice_state {
                Some(state) => state,
                None => return Poll::Pending,
            };

            let tip = forckchoice_state.head_block_hash;
            let next_state = match this.pipeline_state.take().expect("pipeline state is set") {
                PipelineState::Running(mut fut) => {
                    match fut.poll_unpin(cx) {
                        Poll::Ready((pipeline, result)) => {
                            // Any pipeline error at this point is fatal.
                            if let Err(error) = result {
                                return Poll::Ready(Err(error.into()))
                            }

                            // Update the state and hashes of the blockchain tree if possible
                            if let Err(error) = this
                                .restore_tree_if_possible(forckchoice_state.finalized_block_hash)
                            {
                                return Poll::Ready(Err(error.into()))
                            }

                            // Get next pipeline state.
                            this.next_pipeline_state(pipeline, tip)
                        }
                        Poll::Pending => {
                            this.pipeline_state = Some(PipelineState::Running(fut));
                            return Poll::Pending
                        }
                    }
                }
                PipelineState::Idle(pipeline) => this.next_pipeline_state(pipeline, tip),
            };
            this.pipeline_state = Some(next_state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use reth_db::mdbx::{test_utils::create_test_rw_db, Env, WriteMap};
    use reth_executor::{
        blockchain_tree::{config::BlockchainTreeConfig, externals::TreeExternals},
        test_utils::TestExecutorFactory,
    };
    use reth_interfaces::{sync::NoopSyncStateUpdate, test_utils::TestConsensus};
    use reth_primitives::{ChainSpecBuilder, H256, MAINNET};
    use reth_stages::{test_utils::TestStages, ExecOutput, PipelineError, StageError};
    use std::{collections::VecDeque, time::Duration};
    use tokio::sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        oneshot::{self, error::TryRecvError},
        watch,
    };

    type TestBeaconConsensusEngine = BeaconConsensusEngine<
        Env<WriteMap>,
        NoopSyncStateUpdate,
        TestConsensus,
        TestExecutorFactory,
    >;

    struct TestEnv {
        // Keep the tip receiver around, so it's not dropped.
        #[allow(dead_code)]
        tip_rx: watch::Receiver<H256>,
        sync_tx: UnboundedSender<BeaconEngineMessage>,
    }

    impl TestEnv {
        fn new(
            tip_rx: watch::Receiver<H256>,
            sync_tx: UnboundedSender<BeaconEngineMessage>,
        ) -> Self {
            Self { tip_rx, sync_tx }
        }

        fn send_new_payload(
            &self,
            block: SealedBlock,
        ) -> oneshot::Receiver<BeaconEngineResult<PayloadStatus>> {
            let (tx, rx) = oneshot::channel();
            self.sync_tx
                .send(BeaconEngineMessage::NewPayload(block, tx))
                .expect("failed to send msg");
            rx
        }

        fn send_forkchoice_updated(
            &self,
            state: ForkchoiceState,
        ) -> oneshot::Receiver<BeaconEngineResult<ForkchoiceUpdated>> {
            let (tx, rx) = oneshot::channel();
            self.sync_tx
                .send(BeaconEngineMessage::ForkchoiceUpdated(state, None, tx))
                .expect("failed to send msg");
            rx
        }
    }

    fn setup_controller(
        exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    ) -> (TestBeaconConsensusEngine, TestEnv) {
        let db = create_test_rw_db();
        let consensus = TestConsensus::default();
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .shanghai_activated()
                .build(),
        );
        let executor_factory = TestExecutorFactory::new(chain_spec.clone());

        // Setup pipeline
        let (tip_tx, tip_rx) = watch::channel(H256::default());
        let pipeline = Pipeline::builder()
            .add_stages(TestStages::new(exec_outputs, Default::default()))
            .with_tip_sender(tip_tx)
            .build();

        // Setup blockchain tree
        let externals = TreeExternals::new(db.clone(), consensus, executor_factory, chain_spec);
        let config = BlockchainTreeConfig::default();
        let tree = BlockchainTree::new(externals, config).expect("failed to create tree");

        let (sync_tx, sync_rx) = unbounded_channel();
        (BeaconConsensusEngine::new(db, pipeline, tree, sync_rx), TestEnv::new(tip_rx, sync_tx))
    }

    fn spawn_controller(
        controller: TestBeaconConsensusEngine,
    ) -> oneshot::Receiver<Result<(), BeaconEngineError>> {
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = controller.await;
            tx.send(result).expect("failed to forward controller result");
        });
        rx
    }

    // Pipeline error is propagated.
    #[tokio::test]
    async fn pipeline_error_is_propagated() {
        let (controller, env) = setup_controller(VecDeque::from([Err(StageError::ChannelClosed)]));
        let rx = spawn_controller(controller);

        let _ = env.send_forkchoice_updated(ForkchoiceState::default());
        assert_matches!(
            rx.await,
            Ok(Err(BeaconEngineError::Pipeline(PipelineError::Stage(StageError::ChannelClosed))))
        );
    }

    // Test that the controller is idle until first forkchoice updated is received.
    #[tokio::test]
    async fn is_idle_until_forkchoice_is_set() {
        let (controller, env) = setup_controller(VecDeque::from([Err(StageError::ChannelClosed)]));
        let mut rx = spawn_controller(controller);

        // controller is idle
        std::thread::sleep(Duration::from_millis(100));
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        // controller is still idle
        let _ = env.send_new_payload(SealedBlock::default());
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        // controller receives a forkchoice state and triggers the pipeline
        let _ = env.send_forkchoice_updated(ForkchoiceState::default());
        assert_matches!(
            rx.await,
            Ok(Err(BeaconEngineError::Pipeline(PipelineError::Stage(StageError::ChannelClosed))))
        );
    }

    // Test that the controller runs the pipeline again if the tree cannot be restored.
    // The controller will propagate the second result (error) only if it runs the pipeline
    // for the second time.
    #[tokio::test]
    async fn runs_pipeline_again_if_tree_not_restored() {
        let (controller, env) = setup_controller(VecDeque::from([
            Ok(ExecOutput { stage_progress: 1, done: true }),
            Err(StageError::ChannelClosed),
        ]));
        let rx = spawn_controller(controller);

        let _ = env.send_forkchoice_updated(ForkchoiceState::default());
        assert_matches!(
            rx.await,
            Ok(Err(BeaconEngineError::Pipeline(PipelineError::Stage(StageError::ChannelClosed))))
        );
    }
}
