use crate::{error::*, ExecInput, ExecOutput, Stage, StageError, UnwindInput};
use futures_util::Future;
use reth_db::database::Database;
use reth_primitives::{listener::EventListeners, stage::StageId, BlockNumber, H256};
use reth_provider::{providers::get_stage_checkpoint, Transaction};
use std::pin::Pin;
use tokio::sync::watch;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

mod builder;
mod ctrl;
mod event;
mod progress;
mod set;
mod sync_metrics;

pub use crate::pipeline::ctrl::ControlFlow;
pub use builder::*;
pub use event::*;
use progress::*;
pub use set::*;
use sync_metrics::*;

/// A container for a queued stage.
pub(crate) type BoxedStage<DB> = Box<dyn Stage<DB>>;

/// The future that returns the owned pipeline and the result of the pipeline run. See
/// [Pipeline::run_as_fut].
pub type PipelineFut<DB> = Pin<Box<dyn Future<Output = PipelineWithResult<DB>> + Send>>;

/// The pipeline type itself with the result of [Pipeline::run_as_fut]
pub type PipelineWithResult<DB> = (Pipeline<DB>, Result<ControlFlow, PipelineError>);

#[cfg_attr(doc, aquamarine::aquamarine)]
/// A staged sync pipeline.
///
/// The pipeline executes queued [stages][Stage] serially. An external component determines the tip
/// of the chain and the pipeline then executes each stage in order from the current local chain tip
/// and the external chain tip. When a stage is executed, it will run until it reaches the chain
/// tip.
///
/// After the entire pipeline has been run, it will run again unless asked to stop (see
/// [Pipeline::set_max_block]).
///
/// ```mermaid
/// graph TB
///   Start[Start]
///   Done[Done]
///   Error[Error]
///   subgraph Unwind
///     StartUnwind(Unwind in reverse order of execution)
///     UnwindStage(Unwind stage)
///     NextStageToUnwind(Next stage)
///   end
///   subgraph Single loop
///     RunLoop(Run loop)
///     NextStage(Next stage)
///     LoopDone(Loop done)
///     subgraph Stage Execution
///       Execute(Execute stage)
///     end
///   end
///   Start --> RunLoop --> NextStage
///   NextStage --> |No stages left| LoopDone
///   NextStage --> |Next stage| Execute
///   Execute --> |Not done| Execute
///   Execute --> |Unwind requested| StartUnwind
///   Execute --> |Done| NextStage
///   Execute --> |Error| Error
///   StartUnwind --> NextStageToUnwind
///   NextStageToUnwind --> |Next stage| UnwindStage
///   NextStageToUnwind --> |No stages left| RunLoop
///   UnwindStage --> |Error| Error
///   UnwindStage --> |Unwound| NextStageToUnwind
///   LoopDone --> |Target block reached| Done
///   LoopDone --> |Target block not reached| RunLoop
/// ```
///
/// # Unwinding
///
/// In case of a validation error (as determined by the consensus engine) in one of the stages, the
/// pipeline will unwind the stages in reverse order of execution. It is also possible to
/// request an unwind manually (see [Pipeline::unwind]).
///
/// # Defaults
///
/// The [DefaultStages](crate::sets::DefaultStages) are used to fully sync reth.
pub struct Pipeline<DB: Database> {
    /// The Database
    db: DB,
    /// All configured stages in the order they will be executed.
    stages: Vec<BoxedStage<DB>>,
    /// The maximum block number to sync to.
    max_block: Option<BlockNumber>,
    /// All listeners for events the pipeline emits.
    listeners: EventListeners<PipelineEvent>,
    /// Keeps track of the progress of the pipeline.
    progress: PipelineProgress,
    /// A receiver for the current chain tip to sync to.
    tip_tx: Option<watch::Sender<H256>>,
    metrics: Metrics,
}

impl<DB> Pipeline<DB>
where
    DB: Database + 'static,
{
    /// Construct a pipeline using a [`PipelineBuilder`].
    pub fn builder() -> PipelineBuilder<DB> {
        PipelineBuilder::default()
    }

    /// Return the minimum pipeline progress
    pub fn minimum_progress(&self) -> Option<u64> {
        self.progress.minimum_progress
    }

    /// Set tip for reverse sync.
    #[track_caller]
    pub fn set_tip(&self, tip: H256) {
        let _ = self.tip_tx.as_ref().expect("tip sender is set").send(tip).map_err(|_| {
            warn!(target: "sync::pipeline", "Chain tip channel closed");
        });
    }

    /// Listen for events on the pipeline.
    pub fn events(&mut self) -> UnboundedReceiverStream<PipelineEvent> {
        self.listeners.new_listener()
    }

    /// Registers progress metrics for each registered stage
    pub fn register_metrics(&mut self) -> Result<(), PipelineError> {
        let tx = self.db.tx()?;
        for stage in &self.stages {
            let stage_id = stage.id();
            self.metrics.stage_checkpoint(
                stage_id,
                get_stage_checkpoint(&tx, stage_id)?.unwrap_or_default(),
                None,
            );
        }
        Ok(())
    }

    /// Consume the pipeline and run it until it reaches the provided tip, if set. Return the
    /// pipeline and its result as a future.
    #[track_caller]
    pub fn run_as_fut(mut self, tip: Option<H256>) -> PipelineFut<DB> {
        // TODO: fix this in a follow up PR. ideally, consensus engine would be responsible for
        // updating metrics.
        let _ = self.register_metrics(); // ignore error
        Box::pin(async move {
            // NOTE: the tip should only be None if we are in continuous sync mode.
            if let Some(tip) = tip {
                self.set_tip(tip);
            }
            let result = self.run_loop().await;
            trace!(target: "sync::pipeline", ?tip, ?result, "Pipeline finished");
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
                    .minimum_progress
                    .zip(self.max_block)
                    .map_or(false, |(progress, target)| progress >= target)
            {
                trace!(
                    target: "sync::pipeline",
                    ?next_action,
                    minimum_progress = ?self.progress.minimum_progress,
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
    pub async fn run_loop(&mut self) -> Result<ControlFlow, PipelineError> {
        let mut previous_stage = None;
        for stage_index in 0..self.stages.len() {
            let stage = &self.stages[stage_index];
            let stage_id = stage.id();

            trace!(target: "sync::pipeline", stage = %stage_id, "Executing stage");
            let next = self
                .execute_stage_to_completion(previous_stage, stage_index)
                .instrument(info_span!("execute", stage = %stage_id))
                .await?;

            trace!(target: "sync::pipeline", stage = %stage_id, ?next, "Completed stage");

            match next {
                ControlFlow::NoProgress { stage_progress } => {
                    if let Some(progress) = stage_progress {
                        self.progress.update(progress);
                    }
                }
                ControlFlow::Continue { progress } => self.progress.update(progress),
                ControlFlow::Unwind { target, bad_block } => {
                    self.unwind(target, Some(bad_block.number)).await?;
                    return Ok(ControlFlow::Unwind { target, bad_block })
                }
            }

            previous_stage = Some((
                stage_id,
                get_stage_checkpoint(&self.db.tx()?, stage_id)?.unwrap_or_default().block_number,
            ));
        }

        Ok(self.progress.next_ctrl())
    }

    /// Unwind the stages to the target block.
    ///
    /// If the unwind is due to a bad block the number of that block should be specified.
    pub async fn unwind(
        &mut self,
        to: BlockNumber,
        bad_block: Option<BlockNumber>,
    ) -> Result<(), PipelineError> {
        // Unwind stages in reverse order of execution
        let unwind_pipeline = self.stages.iter_mut().rev();

        let mut tx = Transaction::new(&self.db)?;

        for stage in unwind_pipeline {
            let stage_id = stage.id();
            let span = info_span!("Unwinding", stage = %stage_id);
            let _enter = span.enter();

            let mut checkpoint = tx.get_stage_checkpoint(stage_id)?.unwrap_or_default();
            if checkpoint.block_number < to {
                debug!(target: "sync::pipeline", from = %checkpoint, %to, "Unwind point too far for stage");
                self.listeners.notify(PipelineEvent::Skipped { stage_id });
                continue
            }

            debug!(target: "sync::pipeline", from = %checkpoint, %to, ?bad_block, "Starting unwind");
            while checkpoint.block_number > to {
                let input = UnwindInput { checkpoint, unwind_to: to, bad_block };
                self.listeners.notify(PipelineEvent::Unwinding { stage_id, input });

                let output = stage.unwind(&mut tx, input).await;
                match output {
                    Ok(unwind_output) => {
                        checkpoint = unwind_output.checkpoint;
                        self.metrics.stage_checkpoint(
                            stage_id, checkpoint,
                            // We assume it was set in the previous execute iteration, so it
                            // doesn't change when we unwind.
                            None,
                        );
                        tx.save_stage_checkpoint(stage_id, checkpoint)?;

                        self.listeners
                            .notify(PipelineEvent::Unwound { stage_id, result: unwind_output });

                        tx.commit()?;
                    }
                    Err(err) => {
                        self.listeners.notify(PipelineEvent::Error { stage_id });
                        return Err(PipelineError::Stage(StageError::Fatal(Box::new(err))))
                    }
                }
            }
        }

        Ok(())
    }

    async fn execute_stage_to_completion(
        &mut self,
        previous_stage: Option<(StageId, BlockNumber)>,
        stage_index: usize,
    ) -> Result<ControlFlow, PipelineError> {
        let total_stages = self.stages.len();

        let stage = &mut self.stages[stage_index];
        let stage_id = stage.id();
        let mut made_progress = false;

        loop {
            let mut tx = Transaction::new(&self.db)?;

            let prev_checkpoint = tx.get_stage_checkpoint(stage_id)?;

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
                self.listeners.notify(PipelineEvent::Skipped { stage_id });

                // We reached the maximum block, so we skip the stage
                return Ok(ControlFlow::NoProgress {
                    stage_progress: prev_checkpoint.map(|progress| progress.block_number),
                })
            }

            self.listeners.notify(PipelineEvent::Running {
                pipeline_position: stage_index + 1,
                pipeline_total: total_stages,
                stage_id,
                checkpoint: prev_checkpoint,
            });

            match stage
                .execute(&mut tx, ExecInput { previous_stage, checkpoint: prev_checkpoint })
                .await
            {
                Ok(out @ ExecOutput { checkpoint, done }) => {
                    made_progress |=
                        checkpoint.block_number != prev_checkpoint.unwrap_or_default().block_number;
                    info!(
                        target: "sync::pipeline",
                        stage = %stage_id,
                        progress = checkpoint.block_number,
                        %checkpoint,
                        %done,
                        "Stage committed progress"
                    );
                    self.metrics.stage_checkpoint(
                        stage_id,
                        checkpoint,
                        self.max_block.or(previous_stage.map(|(_, block_number)| block_number)),
                    );
                    tx.save_stage_checkpoint(stage_id, checkpoint)?;

                    self.listeners.notify(PipelineEvent::Ran {
                        pipeline_position: stage_index + 1,
                        pipeline_total: total_stages,
                        stage_id,
                        result: out.clone(),
                    });

                    // TODO: Make the commit interval configurable
                    tx.commit()?;

                    if done {
                        let stage_progress = checkpoint.block_number;
                        return Ok(if made_progress {
                            ControlFlow::Continue { progress: stage_progress }
                        } else {
                            ControlFlow::NoProgress { stage_progress: Some(stage_progress) }
                        })
                    }
                }
                Err(err) => {
                    self.listeners.notify(PipelineEvent::Error { stage_id });

                    let out = if let StageError::Validation { block, error } = err {
                        warn!(
                            target: "sync::pipeline",
                            stage = %stage_id,
                            bad_block = %block.number,
                            "Stage encountered a validation error: {error}"
                        );

                        // We unwind because of a validation error. If the unwind itself fails,
                        // we bail entirely, otherwise we restart the execution loop from the
                        // beginning.
                        Ok(ControlFlow::Unwind {
                            target: prev_checkpoint.unwrap_or_default().block_number,
                            bad_block: block,
                        })
                    } else if err.is_fatal() {
                        error!(
                            target: "sync::pipeline",
                            stage = %stage_id,
                            "Stage encountered a fatal error: {err}."
                        );
                        Err(err.into())
                    } else {
                        // On other errors we assume they are recoverable if we discard the
                        // transaction and run the stage again.
                        warn!(
                            target: "sync::pipeline",
                            stage = %stage_id,
                            "Stage encountered a non-fatal error: {err}. Retrying..."
                        );
                        continue
                    };
                    return out
                }
            }
        }
    }
}

impl<DB: Database> std::fmt::Debug for Pipeline<DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("stages", &self.stages.iter().map(|stage| stage.id()).collect::<Vec<StageId>>())
            .field("max_block", &self.max_block)
            .field("listeners", &self.listeners)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestStage, UnwindOutput};
    use assert_matches::assert_matches;
    use reth_db::mdbx::{self, test_utils, EnvKind};
    use reth_interfaces::{
        consensus, provider::ProviderError, test_utils::generators::random_header,
    };
    use reth_primitives::stage::StageCheckpoint;
    use tokio_stream::StreamExt;

    #[test]
    fn record_progress_calculates_outliers() {
        let mut progress = PipelineProgress::default();

        progress.update(10);
        assert_eq!(progress.minimum_progress, Some(10));
        assert_eq!(progress.maximum_progress, Some(10));

        progress.update(20);
        assert_eq!(progress.minimum_progress, Some(10));
        assert_eq!(progress.maximum_progress, Some(20));

        progress.update(1);
        assert_eq!(progress.minimum_progress, Some(1));
        assert_eq!(progress.maximum_progress, Some(20));
    }

    #[test]
    fn progress_ctrl_flow() {
        let mut progress = PipelineProgress::default();

        assert_eq!(progress.next_ctrl(), ControlFlow::NoProgress { stage_progress: None });

        progress.update(1);
        assert_eq!(progress.next_ctrl(), ControlFlow::Continue { progress: 1 });
    }

    /// Runs a simple pipeline.
    #[tokio::test]
    async fn run_pipeline() {
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);

        let mut pipeline = Pipeline::builder()
            .add_stage(
                TestStage::new(StageId::Other("A"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(20), done: true })),
            )
            .add_stage(
                TestStage::new(StageId::Other("B"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .with_max_block(10)
            .build(db);
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            pipeline.run().await.unwrap();
        });

        // Check that the stages were run in order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Running {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(20), done: true },
                },
                PipelineEvent::Running {
                    pipeline_position: 2,
                    pipeline_total: 2,
                    stage_id: StageId::Other("B"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 2,
                    pipeline_total: 2,
                    stage_id: StageId::Other("B"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
            ]
        );
    }

    /// Unwinds a simple pipeline.
    #[tokio::test]
    async fn unwind_pipeline() {
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);

        let mut pipeline = Pipeline::builder()
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
            .build(db);
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            // Sync first
            pipeline.run().await.expect("Could not run pipeline");

            // Unwind
            pipeline.unwind(1, None).await.expect("Could not unwind pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                // Executing
                PipelineEvent::Running {
                    pipeline_position: 1,
                    pipeline_total: 3,
                    stage_id: StageId::Other("A"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 1,
                    pipeline_total: 3,
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(100), done: true },
                },
                PipelineEvent::Running {
                    pipeline_position: 2,
                    pipeline_total: 3,
                    stage_id: StageId::Other("B"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 2,
                    pipeline_total: 3,
                    stage_id: StageId::Other("B"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                PipelineEvent::Running {
                    pipeline_position: 3,
                    pipeline_total: 3,
                    stage_id: StageId::Other("C"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 3,
                    pipeline_total: 3,
                    stage_id: StageId::Other("C"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(20), done: true },
                },
                // Unwinding
                PipelineEvent::Unwinding {
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
                PipelineEvent::Unwinding {
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
                PipelineEvent::Unwinding {
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
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);

        let mut pipeline = Pipeline::builder()
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
            .build(db);
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            // Sync first
            pipeline.run().await.expect("Could not run pipeline");

            // Unwind
            pipeline.unwind(50, None).await.expect("Could not unwind pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                // Executing
                PipelineEvent::Running {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(100), done: true },
                },
                PipelineEvent::Running {
                    pipeline_position: 2,
                    pipeline_total: 2,
                    stage_id: StageId::Other("B"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 2,
                    pipeline_total: 2,
                    stage_id: StageId::Other("B"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                // Unwinding
                // Nothing to unwind in stage "B"
                PipelineEvent::Skipped { stage_id: StageId::Other("B") },
                PipelineEvent::Unwinding {
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
    /// - Stage B unwinds to it's previous progress, block 0 but since it is still at block 0, it is
    ///   skipped entirely (there is nothing to unwind)
    /// - Stage A unwinds to it's previous progress, block 0
    /// - Stage A syncs back up to block 10
    /// - Stage B syncs to block 10
    /// - The pipeline finishes
    #[tokio::test]
    async fn run_pipeline_with_unwind() {
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);

        let mut pipeline = Pipeline::builder()
            .add_stage(
                TestStage::new(StageId::Other("A"))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(0) }))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .add_stage(
                TestStage::new(StageId::Other("B"))
                    .add_exec(Err(StageError::Validation {
                        block: random_header(5, Default::default()),
                        error: consensus::ConsensusError::BaseFeeMissing,
                    }))
                    .add_unwind(Ok(UnwindOutput { checkpoint: StageCheckpoint::new(0) }))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .with_max_block(10)
            .build(db);
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            pipeline.run().await.expect("Could not run pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Running {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                PipelineEvent::Running {
                    pipeline_position: 2,
                    pipeline_total: 2,
                    stage_id: StageId::Other("B"),
                    checkpoint: None
                },
                PipelineEvent::Error { stage_id: StageId::Other("B") },
                PipelineEvent::Unwinding {
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
                PipelineEvent::Running {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    checkpoint: Some(StageCheckpoint::new(0))
                },
                PipelineEvent::Ran {
                    pipeline_position: 1,
                    pipeline_total: 2,
                    stage_id: StageId::Other("A"),
                    result: ExecOutput { checkpoint: StageCheckpoint::new(10), done: true },
                },
                PipelineEvent::Running {
                    pipeline_position: 2,
                    pipeline_total: 2,
                    stage_id: StageId::Other("B"),
                    checkpoint: None
                },
                PipelineEvent::Ran {
                    pipeline_position: 2,
                    pipeline_total: 2,
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
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);
        let mut pipeline = Pipeline::builder()
            .add_stage(
                TestStage::new(StageId::Other("NonFatal"))
                    .add_exec(Err(StageError::Recoverable(Box::new(std::fmt::Error))))
                    .add_exec(Ok(ExecOutput { checkpoint: StageCheckpoint::new(10), done: true })),
            )
            .with_max_block(10)
            .build(db);
        let result = pipeline.run().await;
        assert_matches!(result, Ok(()));

        // Fatal
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);
        let mut pipeline = Pipeline::builder()
            .add_stage(TestStage::new(StageId::Other("Fatal")).add_exec(Err(
                StageError::DatabaseIntegrity(ProviderError::BlockBodyIndicesNotFound(5)),
            )))
            .build(db);
        let result = pipeline.run().await;
        assert_matches!(
            result,
            Err(PipelineError::Stage(StageError::DatabaseIntegrity(
                ProviderError::BlockBodyIndicesNotFound(5)
            )))
        );
    }
}
