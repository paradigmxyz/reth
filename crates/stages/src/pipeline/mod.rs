use crate::{error::*, util::opt, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput};
use metrics::Gauge;
use reth_db::database::Database;
use reth_interfaces::sync::{SyncState, SyncStateUpdater};
use reth_metrics_derive::Metrics;
use reth_primitives::BlockNumber;
use reth_provider::Transaction;
use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    ops::Deref,
    sync::Arc,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

mod builder;
mod ctrl;
mod event;
mod set;

pub use builder::*;
use ctrl::*;
pub use event::*;
pub use set::*;

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
pub struct Pipeline<DB: Database, U: SyncStateUpdater> {
    stages: Vec<BoxedStage<DB>>,
    max_block: Option<BlockNumber>,
    listeners: PipelineEventListeners,
    sync_state_updater: Option<U>,
    progress: PipelineProgress,
    metrics: Metrics,
}

impl<DB: Database, U: SyncStateUpdater> Default for Pipeline<DB, U> {
    fn default() -> Self {
        Self {
            stages: Vec::new(),
            max_block: None,
            listeners: PipelineEventListeners::default(),
            sync_state_updater: None,
            progress: PipelineProgress::default(),
            metrics: Metrics::default(),
        }
    }
}

impl<DB: Database, U: SyncStateUpdater> Debug for Pipeline<DB, U> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("stages", &self.stages.iter().map(|stage| stage.id()).collect::<Vec<StageId>>())
            .field("max_block", &self.max_block)
            .finish()
    }
}

impl<DB: Database, U: SyncStateUpdater> Pipeline<DB, U> {
    /// Construct a pipeline using a [`PipelineBuilder`].
    pub fn builder() -> PipelineBuilder<DB, U> {
        PipelineBuilder::default()
    }

    /// Listen for events on the pipeline.
    pub fn events(&mut self) -> UnboundedReceiverStream<PipelineEvent> {
        self.listeners.new_listener()
    }

    /// Registers progress metrics for each registered stage
    fn register_metrics(&mut self, db: Arc<DB>) {
        for stage in &self.stages {
            let stage_id = stage.id();
            self.metrics.stage_checkpoint(
                stage_id,
                db.view(|tx| stage_id.get_progress(tx).ok().flatten().unwrap_or_default())
                    .ok()
                    .unwrap_or_default(),
            );
        }
    }

    /// Run the pipeline in an infinite loop. Will terminate early if the user has specified
    /// a `max_block` in the pipeline.
    pub async fn run(&mut self, db: Arc<DB>) -> Result<(), PipelineError> {
        self.register_metrics(db.clone());

        loop {
            let next_action = self.run_loop(db.as_ref()).await?;

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
    async fn run_loop(&mut self, db: &DB) -> Result<ControlFlow, PipelineError> {
        let mut previous_stage = None;
        for stage_index in 0..self.stages.len() {
            let stage = &self.stages[stage_index];
            let stage_id = stage.id();

            // Update sync state
            if let Some(ref updater) = self.sync_state_updater {
                let state = self.progress.current_sync_state(stage_id.is_downloading_stage());
                updater.update_sync_state(state);
            }

            trace!(target: "sync::pipeline", stage = %stage_id, "Executing stage");
            let next = self
                .execute_stage_to_completion(db, previous_stage, stage_index)
                .instrument(info_span!("execute", stage = %stage_id))
                .await?;

            match next {
                ControlFlow::NoProgress { stage_progress } => {
                    if let Some(progress) = stage_progress {
                        self.progress.update(progress);
                    }
                }
                ControlFlow::Continue { progress } => self.progress.update(progress),
                ControlFlow::Unwind { target, bad_block } => {
                    // reset the sync state
                    if let Some(ref updater) = self.sync_state_updater {
                        updater.update_sync_state(SyncState::Downloading { target_block: target });
                    }
                    self.unwind(db, target, bad_block).await?;
                    return Ok(ControlFlow::Unwind { target, bad_block })
                }
            }

            previous_stage =
                Some((stage_id, db.view(|tx| stage_id.get_progress(tx))??.unwrap_or_default()));
        }

        Ok(self.progress.next_ctrl())
    }

    /// Unwind the stages to the target block.
    ///
    /// If the unwind is due to a bad block the number of that block should be specified.
    pub async fn unwind(
        &mut self,
        db: &DB,
        to: BlockNumber,
        bad_block: Option<BlockNumber>,
    ) -> Result<(), PipelineError> {
        // Unwind stages in reverse order of execution
        let unwind_pipeline = self.stages.iter_mut().rev();

        let mut tx = Transaction::new(db)?;

        for stage in unwind_pipeline {
            let stage_id = stage.id();
            let span = info_span!("Unwinding", stage = %stage_id);
            let _enter = span.enter();

            let mut stage_progress = stage_id.get_progress(tx.deref())?.unwrap_or_default();
            if stage_progress < to {
                debug!(from = %stage_progress, %to, "Unwind point too far for stage");
                self.listeners.notify(PipelineEvent::Skipped { stage_id });
                return Ok(())
            }

            debug!(from = %stage_progress, %to, ?bad_block, "Starting unwind");
            while stage_progress > to {
                let input = UnwindInput { stage_progress, unwind_to: to, bad_block };
                self.listeners.notify(PipelineEvent::Unwinding { stage_id, input });

                let output = stage.unwind(&mut tx, input).await;
                match output {
                    Ok(unwind_output) => {
                        stage_progress = unwind_output.stage_progress;
                        self.metrics.stage_checkpoint(stage_id, stage_progress);
                        stage_id.save_progress(tx.deref(), stage_progress)?;

                        self.listeners
                            .notify(PipelineEvent::Unwound { stage_id, result: unwind_output });
                    }
                    Err(err) => {
                        self.listeners.notify(PipelineEvent::Error { stage_id });
                        return Err(PipelineError::Stage(StageError::Fatal(Box::new(err))))
                    }
                }
            }
        }

        tx.commit()?;
        Ok(())
    }

    async fn execute_stage_to_completion(
        &mut self,
        db: &DB,
        previous_stage: Option<(StageId, BlockNumber)>,
        stage_index: usize,
    ) -> Result<ControlFlow, PipelineError> {
        let stage = &mut self.stages[stage_index];
        let stage_id = stage.id();
        let mut made_progress = false;
        loop {
            let mut tx = Transaction::new(db)?;

            let prev_progress = stage_id.get_progress(tx.deref())?;

            let stage_reached_max_block = prev_progress
                .zip(self.max_block)
                .map_or(false, |(prev_progress, target)| prev_progress >= target);
            if stage_reached_max_block {
                warn!(
                    target: "sync::pipeline",
                    stage = %stage_id,
                    "Stage reached maximum block, skipping."
                );
                self.listeners.notify(PipelineEvent::Skipped { stage_id });

                // We reached the maximum block, so we skip the stage
                return Ok(ControlFlow::NoProgress { stage_progress: prev_progress })
            }

            self.listeners
                .notify(PipelineEvent::Running { stage_id, stage_progress: prev_progress });

            match stage
                .execute(&mut tx, ExecInput { previous_stage, stage_progress: prev_progress })
                .await
            {
                Ok(out @ ExecOutput { stage_progress, done }) => {
                    made_progress |= stage_progress != prev_progress.unwrap_or_default();
                    info!(
                        target: "sync::pipeline",
                        stage = %stage_id,
                        %stage_progress,
                        %done,
                        "Stage made progress"
                    );
                    self.metrics.stage_checkpoint(stage_id, stage_progress);
                    stage_id.save_progress(tx.deref(), stage_progress)?;

                    self.listeners.notify(PipelineEvent::Ran { stage_id, result: out.clone() });

                    // TODO: Make the commit interval configurable
                    tx.commit()?;

                    if done {
                        return Ok(if made_progress {
                            ControlFlow::Continue { progress: stage_progress }
                        } else {
                            ControlFlow::NoProgress { stage_progress: Some(stage_progress) }
                        })
                    }
                }
                Err(err) => {
                    self.listeners.notify(PipelineEvent::Error { stage_id });

                    return if let StageError::Validation { block, error } = err {
                        warn!(
                            target: "sync::pipeline",
                            stage = %stage_id,
                            bad_block = %block,
                            "Stage encountered a validation error: {error}"
                        );

                        // We unwind because of a validation error. If the unwind itself fails,
                        // we bail entirely, otherwise we restart the execution loop from the
                        // beginning.
                        Ok(ControlFlow::Unwind {
                            target: prev_progress.unwrap_or_default(),
                            bad_block: Some(block),
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
                            "Stage encountered a non-fatal error: {err}. Retrying"
                        );
                        continue
                    }
                }
            }
        }
    }
}

#[derive(Metrics)]
#[metrics(scope = "sync")]
struct StageMetrics {
    /// The block number of the last commit for a stage.
    checkpoint: Gauge,
}

#[derive(Default)]
struct Metrics {
    checkpoints: HashMap<StageId, StageMetrics>,
}

impl Metrics {
    fn stage_checkpoint(&mut self, stage_id: StageId, progress: u64) {
        self.checkpoints
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]))
            .checkpoint
            .set(progress as f64);
    }
}

#[derive(Debug, Default)]
struct PipelineProgress {
    /// The progress of the current stage
    pub(crate) progress: Option<BlockNumber>,
    /// The maximum progress achieved by any stage during the execution of the pipeline.
    pub(crate) maximum_progress: Option<BlockNumber>,
    /// The minimum progress achieved by any stage during the execution of the pipeline.
    pub(crate) minimum_progress: Option<BlockNumber>,
}

impl PipelineProgress {
    fn update(&mut self, progress: BlockNumber) {
        self.progress = Some(progress);
        self.minimum_progress = opt::min(self.minimum_progress, progress);
        self.maximum_progress = opt::max(self.maximum_progress, progress);
    }

    /// Create a sync state from pipeline progress.
    fn current_sync_state(&self, downloading: bool) -> SyncState {
        match self.progress {
            Some(progress) if downloading => SyncState::Downloading { target_block: progress },
            Some(progress) => SyncState::Executing { target_block: progress },
            None => SyncState::Idle,
        }
    }

    /// Get next control flow step
    fn next_ctrl(&self) -> ControlFlow {
        match self.progress {
            Some(progress) => ControlFlow::Continue { progress },
            None => ControlFlow::NoProgress { stage_progress: None },
        }
    }
}

/// A container for a queued stage.
pub(crate) type BoxedStage<DB> = Box<dyn Stage<DB>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StageId, UnwindOutput};
    use assert_matches::assert_matches;
    use reth_db::mdbx::{self, test_utils, EnvKind};
    use reth_interfaces::{consensus, provider::ProviderError, sync::NoopSyncStateUpdate};
    use tokio_stream::StreamExt;
    use utils::TestStage;

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
    fn sync_states() {
        let mut progress = PipelineProgress::default();

        // no progress, so we're idle
        assert_eq!(progress.current_sync_state(false), SyncState::Idle);
        assert_eq!(progress.current_sync_state(true), SyncState::Idle);

        // progress and downloading/executing
        progress.update(1);
        assert_eq!(progress.current_sync_state(true), SyncState::Downloading { target_block: 1 });
        assert_eq!(progress.current_sync_state(false), SyncState::Executing { target_block: 1 });
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

        let mut pipeline: Pipeline<_, NoopSyncStateUpdate> = Pipeline::builder()
            .add_stage(
                TestStage::new(StageId("A"))
                    .add_exec(Ok(ExecOutput { stage_progress: 20, done: true })),
            )
            .add_stage(
                TestStage::new(StageId("B"))
                    .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
            )
            .with_max_block(10)
            .build();
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            pipeline.run(db).await.unwrap();
        });

        // Check that the stages were run in order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Running { stage_id: StageId("A"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("A"),
                    result: ExecOutput { stage_progress: 20, done: true },
                },
                PipelineEvent::Running { stage_id: StageId("B"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("B"),
                    result: ExecOutput { stage_progress: 10, done: true },
                },
            ]
        );
    }

    /// Unwinds a simple pipeline.
    #[tokio::test]
    async fn unwind_pipeline() {
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);

        let mut pipeline: Pipeline<_, NoopSyncStateUpdate> = Pipeline::builder()
            .add_stage(
                TestStage::new(StageId("A"))
                    .add_exec(Ok(ExecOutput { stage_progress: 100, done: true }))
                    .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
            )
            .add_stage(
                TestStage::new(StageId("B"))
                    .add_exec(Ok(ExecOutput { stage_progress: 10, done: true }))
                    .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
            )
            .add_stage(
                TestStage::new(StageId("C"))
                    .add_exec(Ok(ExecOutput { stage_progress: 20, done: true }))
                    .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
            )
            .with_max_block(10)
            .build();
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            // Sync first
            pipeline.run(db.clone()).await.expect("Could not run pipeline");

            // Unwind
            pipeline.unwind(&db, 1, None).await.expect("Could not unwind pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                // Executing
                PipelineEvent::Running { stage_id: StageId("A"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("A"),
                    result: ExecOutput { stage_progress: 100, done: true },
                },
                PipelineEvent::Running { stage_id: StageId("B"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("B"),
                    result: ExecOutput { stage_progress: 10, done: true },
                },
                PipelineEvent::Running { stage_id: StageId("C"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("C"),
                    result: ExecOutput { stage_progress: 20, done: true },
                },
                // Unwinding
                PipelineEvent::Unwinding {
                    stage_id: StageId("C"),
                    input: UnwindInput { stage_progress: 20, unwind_to: 1, bad_block: None }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId("C"),
                    result: UnwindOutput { stage_progress: 1 },
                },
                PipelineEvent::Unwinding {
                    stage_id: StageId("B"),
                    input: UnwindInput { stage_progress: 10, unwind_to: 1, bad_block: None }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId("B"),
                    result: UnwindOutput { stage_progress: 1 },
                },
                PipelineEvent::Unwinding {
                    stage_id: StageId("A"),
                    input: UnwindInput { stage_progress: 100, unwind_to: 1, bad_block: None }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId("A"),
                    result: UnwindOutput { stage_progress: 1 },
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

        let mut pipeline: Pipeline<_, NoopSyncStateUpdate> = Pipeline::builder()
            .add_stage(
                TestStage::new(StageId("A"))
                    .add_exec(Ok(ExecOutput { stage_progress: 10, done: true }))
                    .add_unwind(Ok(UnwindOutput { stage_progress: 0 }))
                    .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
            )
            .add_stage(
                TestStage::new(StageId("B"))
                    .add_exec(Err(StageError::Validation {
                        block: 5,
                        error: consensus::ConsensusError::BaseFeeMissing,
                    }))
                    .add_unwind(Ok(UnwindOutput { stage_progress: 0 }))
                    .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
            )
            .with_max_block(10)
            .build();
        let events = pipeline.events();

        // Run pipeline
        tokio::spawn(async move {
            pipeline.run(db).await.expect("Could not run pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            events.collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Running { stage_id: StageId("A"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("A"),
                    result: ExecOutput { stage_progress: 10, done: true },
                },
                PipelineEvent::Running { stage_id: StageId("B"), stage_progress: None },
                PipelineEvent::Error { stage_id: StageId("B") },
                PipelineEvent::Unwinding {
                    stage_id: StageId("A"),
                    input: UnwindInput { stage_progress: 10, unwind_to: 0, bad_block: Some(5) }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId("A"),
                    result: UnwindOutput { stage_progress: 0 },
                },
                PipelineEvent::Running { stage_id: StageId("A"), stage_progress: Some(0) },
                PipelineEvent::Ran {
                    stage_id: StageId("A"),
                    result: ExecOutput { stage_progress: 10, done: true },
                },
                PipelineEvent::Running { stage_id: StageId("B"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("B"),
                    result: ExecOutput { stage_progress: 10, done: true },
                },
            ]
        );
    }

    /// Checks that the pipeline re-runs stages on non-fatal errors and stops on fatal ones.
    #[tokio::test]
    async fn pipeline_error_handling() {
        // Non-fatal
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);
        let mut pipeline: Pipeline<_, NoopSyncStateUpdate> = Pipeline::builder()
            .add_stage(
                TestStage::new(StageId("NonFatal"))
                    .add_exec(Err(StageError::Recoverable(Box::new(std::fmt::Error))))
                    .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
            )
            .with_max_block(10)
            .build();
        let result = pipeline.run(db).await;
        assert_matches!(result, Ok(()));

        // Fatal
        let db = test_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);
        let mut pipeline: Pipeline<_, NoopSyncStateUpdate> = Pipeline::builder()
            .add_stage(TestStage::new(StageId("Fatal")).add_exec(Err(
                StageError::DatabaseIntegrity(ProviderError::BlockBody { number: 5 }),
            )))
            .build();
        let result = pipeline.run(db).await;
        assert_matches!(
            result,
            Err(PipelineError::Stage(StageError::DatabaseIntegrity(ProviderError::BlockBody {
                number: 5
            })))
        );
    }

    mod utils {
        use super::*;
        use async_trait::async_trait;
        use std::collections::VecDeque;

        pub(crate) struct TestStage {
            id: StageId,
            exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
            unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
        }

        impl TestStage {
            pub(crate) fn new(id: StageId) -> Self {
                Self { id, exec_outputs: VecDeque::new(), unwind_outputs: VecDeque::new() }
            }

            pub(crate) fn add_exec(mut self, output: Result<ExecOutput, StageError>) -> Self {
                self.exec_outputs.push_back(output);
                self
            }

            pub(crate) fn add_unwind(mut self, output: Result<UnwindOutput, StageError>) -> Self {
                self.unwind_outputs.push_back(output);
                self
            }
        }

        #[async_trait]
        impl<DB: Database> Stage<DB> for TestStage {
            fn id(&self) -> StageId {
                self.id
            }

            async fn execute(
                &mut self,
                _: &mut Transaction<'_, DB>,
                _input: ExecInput,
            ) -> Result<ExecOutput, StageError> {
                self.exec_outputs
                    .pop_front()
                    .unwrap_or_else(|| panic!("Test stage {} executed too many times.", self.id))
            }

            async fn unwind(
                &mut self,
                _: &mut Transaction<'_, DB>,
                _input: UnwindInput,
            ) -> Result<UnwindOutput, StageError> {
                self.unwind_outputs
                    .pop_front()
                    .unwrap_or_else(|| panic!("Test stage {} unwound too many times.", self.id))
            }
        }
    }
}
