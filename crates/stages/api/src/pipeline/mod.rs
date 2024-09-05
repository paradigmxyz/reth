mod ctrl;
mod event;
pub use crate::pipeline::ctrl::ControlFlow;
use crate::{PipelineTarget, StageCheckpoint, StageId};
use alloy_primitives::{BlockNumber, B256};
pub use event::*;
use futures_util::Future;
use reth_node_types::NodeTypesWithDB;
use reth_primitives_traits::constants::BEACON_CONSENSUS_REORG_UNWIND_DEPTH;
use reth_provider::{
    providers::ProviderNodeTypes, writer::UnifiedStorageWriter, FinalizedBlockReader,
    FinalizedBlockWriter, ProviderFactory, StageCheckpointReader, StageCheckpointWriter,
    StaticFileProviderFactory,
};
use reth_prune::PrunerBuilder;
use reth_static_file::StaticFileProducer;
use reth_tokio_util::{EventSender, EventStream};
use std::pin::Pin;
use tokio::sync::watch;
use tracing::*;

mod builder;
mod progress;
mod set;

use crate::{
    BlockErrorKind, ExecInput, ExecOutput, MetricEvent, MetricEventsSender, PipelineError, Stage,
    StageError, StageExt, UnwindInput,
};
pub use builder::*;
use progress::*;
use reth_errors::RethResult;
pub use set::*;

/// A container for a queued stage.
pub(crate) type BoxedStage<DB> = Box<dyn Stage<DB>>;

/// The future that returns the owned pipeline and the result of the pipeline run. See
/// [`Pipeline::run_as_fut`].
pub type PipelineFut<N> = Pin<Box<dyn Future<Output = PipelineWithResult<N>> + Send>>;

/// The pipeline type itself with the result of [`Pipeline::run_as_fut`]
pub type PipelineWithResult<N> = (Pipeline<N>, Result<ControlFlow, PipelineError>);

#[cfg_attr(doc, aquamarine::aquamarine)]
/// A staged sync pipeline.
///
/// The pipeline executes queued [stages][Stage] serially. An external component determines the tip
/// of the chain and the pipeline then executes each stage in order from the current local chain tip
/// and the external chain tip. When a stage is executed, it will run until it reaches the chain
/// tip.
///
/// After the entire pipeline has been run, it will run again unless asked to stop (see
/// [`Pipeline::set_max_block`]).
///
/// `include_mmd!("docs/mermaid/pipeline.mmd`")
///
/// # Unwinding
///
/// In case of a validation error (as determined by the consensus engine) in one of the stages, the
/// pipeline will unwind the stages in reverse order of execution. It is also possible to
/// request an unwind manually (see [`Pipeline::unwind`]).
///
/// # Defaults
///
/// The [`DefaultStages`](crate::sets::DefaultStages) are used to fully sync reth.
pub struct Pipeline<N: NodeTypesWithDB> {
    /// Provider factory.
    provider_factory: ProviderFactory<N>,
    /// All configured stages in the order they will be executed.
    stages: Vec<BoxedStage<N::DB>>,
    /// The maximum block number to sync to.
    max_block: Option<BlockNumber>,
    static_file_producer: StaticFileProducer<N>,
    /// Sender for events the pipeline emits.
    event_sender: EventSender<PipelineEvent>,
    /// Keeps track of the progress of the pipeline.
    progress: PipelineProgress,
    /// A receiver for the current chain tip to sync to.
    tip_tx: Option<watch::Sender<B256>>,
    metrics_tx: Option<MetricEventsSender>,
}

impl<N: ProviderNodeTypes> Pipeline<N> {
    /// Construct a pipeline using a [`PipelineBuilder`].
    pub fn builder() -> PipelineBuilder<N::DB> {
        PipelineBuilder::default()
    }

    /// Return the minimum block number achieved by
    /// any stage during the execution of the pipeline.
    pub const fn minimum_block_number(&self) -> Option<u64> {
        self.progress.minimum_block_number
    }

    /// Set tip for reverse sync.
    #[track_caller]
    pub fn set_tip(&self, tip: B256) {
        let _ = self.tip_tx.as_ref().expect("tip sender is set").send(tip).map_err(|_| {
            warn!(target: "sync::pipeline", "Chain tip channel closed");
        });
    }

    /// Listen for events on the pipeline.
    pub fn events(&self) -> EventStream<PipelineEvent> {
        self.event_sender.new_listener()
    }
}

impl<N: ProviderNodeTypes> Pipeline<N> {
    /// Registers progress metrics for each registered stage
    pub fn register_metrics(&mut self) -> Result<(), PipelineError> {
        let Some(metrics_tx) = &mut self.metrics_tx else { return Ok(()) };
        let provider = self.provider_factory.provider()?;

        for stage in &self.stages {
            let stage_id = stage.id();
            let _ = metrics_tx.send(MetricEvent::StageCheckpoint {
                stage_id,
                checkpoint: provider.get_stage_checkpoint(stage_id)?.unwrap_or_default(),
                max_block_number: None,
            });
        }
        Ok(())
    }

    /// Consume the pipeline and run it until it reaches the provided tip, if set. Return the
    /// pipeline and its result as a future.
    #[track_caller]
    pub fn run_as_fut(mut self, target: Option<PipelineTarget>) -> PipelineFut<N> {
        // TODO: fix this in a follow up PR. ideally, consensus engine would be responsible for
        // updating metrics.
        let _ = self.register_metrics(); // ignore error
        Box::pin(async move {
            // NOTE: the tip should only be None if we are in continuous sync mode.
            if let Some(target) = target {
                match target {
                    PipelineTarget::Sync(tip) => self.set_tip(tip),
                    PipelineTarget::Unwind(target) => {
                        if let Err(err) = self.move_to_static_files() {
                            return (self, Err(err.into()))
                        }
                        if let Err(err) = self.unwind(target, None) {
                            return (self, Err(err))
                        }
                        self.progress.update(target);

                        return (self, Ok(ControlFlow::Continue { block_number: target }))
                    }
                }
            }

            let result = self.run_loop().await;
            trace!(target: "sync::pipeline", ?target, ?result, "Pipeline finished");
            (self, result)
        })
    }

    /// Run the pipeline in an infinite loop. Will terminate early if the user has specified
    /// a `max_block` in the pipeline.
    pub async fn run(&mut self) -> Result<(), PipelineError> {
        let _ = self.register_metrics(); // ignore error

        loop {
            let next_action = self.run_loop().await?;

            // Terminate the loop early if it's reached the maximum user
            // configured block.
            if next_action.should_continue() &&
                self.progress
                    .minimum_block_number
                    .zip(self.max_block)
                    .map_or(false, |(progress, target)| progress >= target)
            {
                trace!(
                    target: "sync::pipeline",
                    ?next_action,
                    minimum_block_number = ?self.progress.minimum_block_number,
                    max_block = ?self.max_block,
                    "Terminating pipeline."
                );
                return Ok(())
            }
        }
    }

    /// Performs one pass of the pipeline across all stages. After successful
    /// execution of each stage, it proceeds to commit it to the database.
    ///
    /// If any stage is unsuccessful at execution, we proceed to
    /// unwind. This will undo the progress across the entire pipeline
    /// up to the block that caused the error.
    ///
    /// Returns the control flow after it ran the pipeline.
    /// This will be [`ControlFlow::Continue`] or [`ControlFlow::NoProgress`] of the _last_ stage in
    /// the pipeline (for example the `Finish` stage). Or [`ControlFlow::Unwind`] of the stage
    /// that caused the unwind.
    pub async fn run_loop(&mut self) -> Result<ControlFlow, PipelineError> {
        self.move_to_static_files()?;

        let mut previous_stage = None;
        for stage_index in 0..self.stages.len() {
            let stage = &self.stages[stage_index];
            let stage_id = stage.id();

            trace!(target: "sync::pipeline", stage = %stage_id, "Executing stage");
            let next = self.execute_stage_to_completion(previous_stage, stage_index).await?;

            trace!(target: "sync::pipeline", stage = %stage_id, ?next, "Completed stage");

            match next {
                ControlFlow::NoProgress { block_number } => {
                    if let Some(block_number) = block_number {
                        self.progress.update(block_number);
                    }
                }
                ControlFlow::Continue { block_number } => self.progress.update(block_number),
                ControlFlow::Unwind { target, bad_block } => {
                    self.unwind(target, Some(bad_block.number))?;
                    return Ok(ControlFlow::Unwind { target, bad_block })
                }
            }

            previous_stage = Some(
                self.provider_factory
                    .provider()?
                    .get_stage_checkpoint(stage_id)?
                    .unwrap_or_default()
                    .block_number,
            );
        }

        Ok(self.progress.next_ctrl())
    }

    /// Run [static file producer](StaticFileProducer) and [pruner](reth_prune::Pruner) to **move**
    /// all data from the database to static files for corresponding
    /// [segments](reth_static_file_types::StaticFileSegment), according to their [stage
    /// checkpoints](StageCheckpoint):
    /// - [`StaticFileSegment::Headers`](reth_static_file_types::StaticFileSegment::Headers) ->
    ///   [`StageId::Headers`]
    /// - [`StaticFileSegment::Receipts`](reth_static_file_types::StaticFileSegment::Receipts) ->
    ///   [`StageId::Execution`]
    /// - [`StaticFileSegment::Transactions`](reth_static_file_types::StaticFileSegment::Transactions)
    ///   -> [`StageId::Bodies`]
    ///
    /// CAUTION: This method locks the static file producer Mutex, hence can block the thread if the
    /// lock is occupied.
    pub fn move_to_static_files(&self) -> RethResult<()> {
        // Copies data from database to static files
        let lowest_static_file_height =
            self.static_file_producer.lock().copy_to_static_files()?.min();

        // Deletes data which has been copied to static files.
        if let Some(prune_tip) = lowest_static_file_height {
            // Run the pruner so we don't potentially end up with higher height in the database vs
            // static files during a pipeline unwind
            let mut pruner = PrunerBuilder::new(Default::default())
                .delete_limit(usize::MAX)
                .build_with_provider_factory(self.provider_factory.clone());

            pruner.run(prune_tip)?;
        }

        Ok(())
    }

    /// Unwind the stages to the target block.
    ///
    /// If the unwind is due to a bad block the number of that block should be specified.
    pub fn unwind(
        &mut self,
        to: BlockNumber,
        bad_block: Option<BlockNumber>,
    ) -> Result<(), PipelineError> {
        // Unwind stages in reverse order of execution
        let unwind_pipeline = self.stages.iter_mut().rev();

        let mut provider_rw = self.provider_factory.provider_rw()?;

        for stage in unwind_pipeline {
            let stage_id = stage.id();
            let span = info_span!("Unwinding", stage = %stage_id);
            let _enter = span.enter();

            let mut checkpoint = provider_rw.get_stage_checkpoint(stage_id)?.unwrap_or_default();
            if checkpoint.block_number < to {
                debug!(
                    target: "sync::pipeline",
                    from = %checkpoint.block_number,
                    %to,
                    "Unwind point too far for stage"
                );
                self.event_sender.notify(PipelineEvent::Skipped { stage_id });

                continue
            }

            info!(
                target: "sync::pipeline",
                from = %checkpoint.block_number,
                %to,
                ?bad_block,
                "Starting unwind"
            );
            while checkpoint.block_number > to {
                let input = UnwindInput { checkpoint, unwind_to: to, bad_block };
                self.event_sender.notify(PipelineEvent::Unwind { stage_id, input });

                let output = stage.unwind(&provider_rw, input);
                match output {
                    Ok(unwind_output) => {
                        checkpoint = unwind_output.checkpoint;
                        info!(
                            target: "sync::pipeline",
                            stage = %stage_id,
                            unwind_to = to,
                            progress = checkpoint.block_number,
                            done = checkpoint.block_number == to,
                            "Stage unwound"
                        );
                        if let Some(metrics_tx) = &mut self.metrics_tx {
                            let _ = metrics_tx.send(MetricEvent::StageCheckpoint {
                                stage_id,
                                checkpoint,
                                // We assume it was set in the previous execute iteration, so it
                                // doesn't change when we unwind.
                                max_block_number: None,
                            });
                        }
                        provider_rw.save_stage_checkpoint(stage_id, checkpoint)?;

                        self.event_sender
                            .notify(PipelineEvent::Unwound { stage_id, result: unwind_output });

                        // update finalized block if needed
                        let last_saved_finalized_block_number =
                            provider_rw.last_finalized_block_number()?;

                        // If None, that means the finalized block is not written so we should
                        // always save in that case
                        if last_saved_finalized_block_number.is_none() ||
                            Some(checkpoint.block_number) < last_saved_finalized_block_number
                        {
                            provider_rw.save_finalized_block_number(BlockNumber::from(
                                checkpoint.block_number,
                            ))?;
                        }

                        UnifiedStorageWriter::commit_unwind(
                            provider_rw,
                            self.provider_factory.static_file_provider(),
                        )?;

                        stage.post_unwind_commit()?;

                        provider_rw = self.provider_factory.provider_rw()?;
                    }
                    Err(err) => {
                        self.event_sender.notify(PipelineEvent::Error { stage_id });

                        return Err(PipelineError::Stage(StageError::Fatal(Box::new(err))))
                    }
                }
            }
        }

        Ok(())
    }

    async fn execute_stage_to_completion(
        &mut self,
        previous_stage: Option<BlockNumber>,
        stage_index: usize,
    ) -> Result<ControlFlow, PipelineError> {
        let total_stages = self.stages.len();

        let stage = &mut self.stages[stage_index];
        let stage_id = stage.id();
        let mut made_progress = false;
        let target = self.max_block.or(previous_stage);

        loop {
            let prev_checkpoint = self.provider_factory.get_stage_checkpoint(stage_id)?;

            let stage_reached_max_block = prev_checkpoint
                .zip(self.max_block)
                .map_or(false, |(prev_progress, target)| prev_progress.block_number >= target);
            if stage_reached_max_block {
                warn!(
                    target: "sync::pipeline",
                    stage = %stage_id,
                    max_block = self.max_block,
                    prev_block = prev_checkpoint.map(|progress| progress.block_number),
                    "Stage reached target block, skipping."
                );
                self.event_sender.notify(PipelineEvent::Skipped { stage_id });

                // We reached the maximum block, so we skip the stage
                return Ok(ControlFlow::NoProgress {
                    block_number: prev_checkpoint.map(|progress| progress.block_number),
                })
            }

            let exec_input = ExecInput { target, checkpoint: prev_checkpoint };

            self.event_sender.notify(PipelineEvent::Prepare {
                pipeline_stages_progress: PipelineStagesProgress {
                    current: stage_index + 1,
                    total: total_stages,
                },
                stage_id,
                checkpoint: prev_checkpoint,
                target,
            });

            if let Err(err) = stage.execute_ready(exec_input).await {
                self.event_sender.notify(PipelineEvent::Error { stage_id });

                match on_stage_error(&self.provider_factory, stage_id, prev_checkpoint, err)? {
                    Some(ctrl) => return Ok(ctrl),
                    None => continue,
                };
            }

            let provider_rw = self.provider_factory.provider_rw()?;

            self.event_sender.notify(PipelineEvent::Run {
                pipeline_stages_progress: PipelineStagesProgress {
                    current: stage_index + 1,
                    total: total_stages,
                },
                stage_id,
                checkpoint: prev_checkpoint,
                target,
            });

            match stage.execute(&provider_rw, exec_input) {
                Ok(out @ ExecOutput { checkpoint, done }) => {
                    made_progress |=
                        checkpoint.block_number != prev_checkpoint.unwrap_or_default().block_number;

                    if let Some(metrics_tx) = &mut self.metrics_tx {
                        let _ = metrics_tx.send(MetricEvent::StageCheckpoint {
                            stage_id,
                            checkpoint,
                            max_block_number: target,
                        });
                    }
                    provider_rw.save_stage_checkpoint(stage_id, checkpoint)?;

                    self.event_sender.notify(PipelineEvent::Ran {
                        pipeline_stages_progress: PipelineStagesProgress {
                            current: stage_index + 1,
                            total: total_stages,
                        },
                        stage_id,
                        result: out.clone(),
                    });

                    UnifiedStorageWriter::commit(
                        provider_rw,
                        self.provider_factory.static_file_provider(),
                    )?;

                    if done {
                        let block_number = checkpoint.block_number;
                        return Ok(if made_progress {
                            ControlFlow::Continue { block_number }
                        } else {
                            ControlFlow::NoProgress { block_number: Some(block_number) }
                        })
                    }
                }
                Err(err) => {
                    drop(provider_rw);
                    self.event_sender.notify(PipelineEvent::Error { stage_id });

                    if let Some(ctrl) =
                        on_stage_error(&self.provider_factory, stage_id, prev_checkpoint, err)?
                    {
                        return Ok(ctrl)
                    }
                }
            }
        }
    }
}

fn on_stage_error<N: ProviderNodeTypes>(
    factory: &ProviderFactory<N>,
    stage_id: StageId,
    prev_checkpoint: Option<StageCheckpoint>,
    err: StageError,
) -> Result<Option<ControlFlow>, PipelineError> {
    if let StageError::DetachedHead { local_head, header, error } = err {
        warn!(target: "sync::pipeline", stage = %stage_id, ?local_head, ?header, %error, "Stage encountered detached head");

        // We unwind because of a detached head.
        let unwind_to =
            local_head.number.saturating_sub(BEACON_CONSENSUS_REORG_UNWIND_DEPTH).max(1);
        Ok(Some(ControlFlow::Unwind { target: unwind_to, bad_block: local_head }))
    } else if let StageError::Block { block, error } = err {
        match error {
            BlockErrorKind::Validation(validation_error) => {
                error!(
                    target: "sync::pipeline",
                    stage = %stage_id,
                    bad_block = %block.number,
                    "Stage encountered a validation error: {validation_error}"
                );

                // FIXME: When handling errors, we do not commit the database transaction. This
                // leads to the Merkle stage not clearing its checkpoint, and restarting from an
                // invalid place.
                let provider_rw = factory.provider_rw()?;
                provider_rw.save_stage_checkpoint_progress(StageId::MerkleExecute, vec![])?;
                provider_rw.save_stage_checkpoint(
                    StageId::MerkleExecute,
                    prev_checkpoint.unwrap_or_default(),
                )?;

                UnifiedStorageWriter::commit(provider_rw, factory.static_file_provider())?;

                // We unwind because of a validation error. If the unwind itself
                // fails, we bail entirely,
                // otherwise we restart the execution loop from the
                // beginning.
                Ok(Some(ControlFlow::Unwind {
                    target: prev_checkpoint.unwrap_or_default().block_number,
                    bad_block: block,
                }))
            }
            BlockErrorKind::Execution(execution_error) => {
                error!(
                    target: "sync::pipeline",
                    stage = %stage_id,
                    bad_block = %block.number,
                    "Stage encountered an execution error: {execution_error}"
                );

                // We unwind because of an execution error. If the unwind itself
                // fails, we bail entirely,
                // otherwise we restart
                // the execution loop from the beginning.
                Ok(Some(ControlFlow::Unwind {
                    target: prev_checkpoint.unwrap_or_default().block_number,
                    bad_block: block,
                }))
            }
        }
    } else if let StageError::MissingStaticFileData { block, segment } = err {
        error!(
            target: "sync::pipeline",
            stage = %stage_id,
            bad_block = %block.number,
            segment = %segment,
            "Stage is missing static file data."
        );

        Ok(Some(ControlFlow::Unwind { target: block.number - 1, bad_block: block }))
    } else if err.is_fatal() {
        error!(target: "sync::pipeline", stage = %stage_id, "Stage encountered a fatal error: {err}");
        Err(err.into())
    } else {
        // On other errors we assume they are recoverable if we discard the
        // transaction and run the stage again.
        warn!(
            target: "sync::pipeline",
            stage = %stage_id,
            "Stage encountered a non-fatal error: {err}. Retrying..."
        );
        Ok(None)
    }
}

impl<N: NodeTypesWithDB> std::fmt::Debug for Pipeline<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("stages", &self.stages.iter().map(|stage| stage.id()).collect::<Vec<StageId>>())
            .field("max_block", &self.max_block)
            .field("event_sender", &self.event_sender)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestStage, UnwindOutput};
    use assert_matches::assert_matches;
    use reth_consensus::ConsensusError;
    use reth_errors::ProviderError;
    use reth_provider::test_utils::{create_test_provider_factory, MockNodeTypesWithDB};
    use reth_prune::PruneModes;
    use reth_testing_utils::{generators, generators::random_header};
    use tokio_stream::StreamExt;

    #[test]
    fn record_progress_calculates_outliers() {
        let mut progress = PipelineProgress::default();

        progress.update(10);
        assert_eq!(progress.minimum_block_number, Some(10));
        assert_eq!(progress.maximum_block_number, Some(10));

        progress.update(20);
        assert_eq!(progress.minimum_block_number, Some(10));
        assert_eq!(progress.maximum_block_number, Some(20));

        progress.update(1);
        assert_eq!(progress.minimum_block_number, Some(1));
        assert_eq!(progress.maximum_block_number, Some(20));
    }

    #[test]
    fn progress_ctrl_flow() {
        let mut progress = PipelineProgress::default();

        assert_eq!(progress.next_ctrl(), ControlFlow::NoProgress { block_number: None });

        progress.update(1);
        assert_eq!(progress.next_ctrl(), ControlFlow::Continue { block_number: 1 });
    }

    /// Runs a simple pipeline.
    #[tokio::test]
    async fn run_pipeline() {
        let provider_factory = create_test_provider_factory();

        let mut pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stage(
                TestStage::new(StageId::Other("A"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(20), done: true })),
            )
            .add_stage(
                TestStage::new(StageId::Other("B"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .with_max_block(10)
            .build(
                provider_factory.clone(),
                StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
            );
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            pipeline.run().await.unwrap();
        });

        // Check that the stages were run in order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(20), done: true },
                },
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
            ]
        );
    }

    /// Unwinds a simple pipeline.
    #[tokio::test]
    async fn unwind_pipeline() {
        let provider_factory = create_test_provider_factory();

        let mut pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stage(
                TestStage::new(StageId::Other("A"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(100), done: true }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(1) })),
            )
            .add_stage(
                TestStage::new(StageId::Other("B"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(1) })),
            )
            .add_stage(
                TestStage::new(StageId::Other("C"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(20), done: true }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(1) })),
            )
            .with_max_block(10)
            .build(
                provider_factory.clone(),
                StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
            );
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            // Sync first
            pipeline.run().await.expect("Could not run pipeline");

            // Unwind
            pipeline.unwind(1, None).expect("Could not unwind pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                // Executing
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 3 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 3 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 3 },
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(100), done: true },
                },
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 3 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 3 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 3 },
                    stage_id: StageId::Other("B"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 3, total: 3 },
                    stage_id: StageId::Other("C"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 3, total: 3 },
                    stage_id: StageId::Other("C"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 3, total: 3 },
                    stage_id: StageId::Other("C"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(20), done: true },
                },
                // Unwinding
                PipelineEvent::Unwind {
                    stage_id: StageId::Other("C"),
                    input: UnwindInput {
                        checkpoint: StageCheckpoint::new(20),
                        unwind_to: 1,
                        bad_block: None
                    }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId::Other("C"),
                    result: UnwindOutput { checkpoint: StageCheckpoint::new(1) },
                },
                PipelineEvent::Unwind {
                    stage_id: StageId::Other("B"),
                    input: UnwindInput {
                        checkpoint: StageCheckpoint::new(10),
                        unwind_to: 1,
                        bad_block: None
                    }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId::Other("B"),
                    result: UnwindOutput { checkpoint: StageCheckpoint::new(1) },
                },
                PipelineEvent::Unwind {
                    stage_id: StageId::Other("A"),
                    input: UnwindInput {
                        checkpoint: StageCheckpoint::new(100),
                        unwind_to: 1,
                        bad_block: None
                    }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId::Other("A"),
                    result: UnwindOutput { checkpoint: StageCheckpoint::new(1) },
                },
            ]
        );
    }

    /// Unwinds a pipeline with intermediate progress.
    #[tokio::test]
    async fn unwind_pipeline_with_intermediate_progress() {
        let provider_factory = create_test_provider_factory();

        let mut pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stage(
                TestStage::new(StageId::Other("A"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(100), done: true }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(50) })),
            )
            .add_stage(
                TestStage::new(StageId::Other("B"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .with_max_block(10)
            .build(
                provider_factory.clone(),
                StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
            );
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            // Sync first
            pipeline.run().await.expect("Could not run pipeline");

            // Unwind
            pipeline.unwind(50, None).expect("Could not unwind pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                // Executing
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(100), done: true },
                },
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                // Unwinding
                // Nothing to unwind in stage "B"
                PipelineEvent::Skipped { stage_id: StageId::Other("B") },
                PipelineEvent::Unwind {
                    stage_id: StageId::Other("A"),
                    input: UnwindInput {
                        checkpoint: StageCheckpoint::new(100),
                        unwind_to: 50,
                        bad_block: None
                    }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId::Other("A"),
                    result: UnwindOutput { checkpoint: StageCheckpoint::new(50) },
                },
            ]
        );
    }

    /// Runs a pipeline that unwinds during sync.
    ///
    /// The flow is:
    ///
    /// - Stage A syncs to block 10
    /// - Stage B triggers an unwind, marking block 5 as bad
    /// - Stage B unwinds to its previous progress, block 0 but since it is still at block 0, it is
    ///   skipped entirely (there is nothing to unwind)
    /// - Stage A unwinds to its previous progress, block 0
    /// - Stage A syncs back up to block 10
    /// - Stage B syncs to block 10
    /// - The pipeline finishes
    #[tokio::test]
    async fn run_pipeline_with_unwind() {
        let provider_factory = create_test_provider_factory();

        let mut pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stage(
                TestStage::new(StageId::Other("A"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(0) }))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .add_stage(
                TestStage::new(StageId::Other("B"))
                    .add_exec(Err(StageError::Block {
                        block: Box::new(random_header(
                            &mut generators::rng(),
                            5,
                            Default::default(),
                        )),
                        error: BlockErrorKind::Validation(ConsensusError::BaseFeeMissing),
                    }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(0) }))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .with_max_block(10)
            .build(
                provider_factory.clone(),
                StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
            );
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            pipeline.run().await.expect("Could not run pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Error { stage_id: StageId::Other("B") },
                PipelineEvent::Unwind {
                    stage_id: StageId::Other("A"),
                    input: UnwindInput {
                        checkpoint: StageCheckpoint::new(10),
                        unwind_to: 0,
                        bad_block: Some(5)
                    }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId::Other("A"),
                    result: UnwindOutput { checkpoint: StageCheckpoint::new(0) },
                },
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: Some(StageCheckpoint::new(0)),
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    checkpoint: Some(StageCheckpoint::new(0)),
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 1, total: 2 },
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                PipelineEvent::Prepare {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Run {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    checkpoint: None,
                    target: Some(10),
                },
                PipelineEvent::Ran {
                    pipeline_stages_progress: PipelineStagesProgress { current: 2, total: 2 },
                    stage_id: StageId::Other("B"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
            ]
        );
    }

    /// Checks that the pipeline re-runs stages on non-fatal errors and stops on fatal ones.
    #[tokio::test]
    async fn pipeline_error_handling() {
        // Non-fatal
        let provider_factory = create_test_provider_factory();
        let mut pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stage(
                TestStage::new(StageId::Other("NonFatal"))
                    .add_exec(Err(StageError::Recoverable(Box::new(std::fmt::Error))))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .with_max_block(10)
            .build(
                provider_factory.clone(),
                StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
            );
        let result = pipeline.run().await;
        assert_matches!(result, Ok(()));

        // Fatal
        let provider_factory = create_test_provider_factory();
        let mut pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stage(TestStage::new(StageId::Other("Fatal")).add_exec(Err(
                StageError::DatabaseIntegrity(ProviderError::BlockBodyIndicesNotFound(5)),
            )))
            .build(
                provider_factory.clone(),
                StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
            );
        let result = pipeline.run().await;
        assert_matches!(
            result,
            Err(PipelineError::Stage(StageError::DatabaseIntegrity(
                ProviderError::BlockBodyIndicesNotFound(5)
            )))
        );
    }
}
