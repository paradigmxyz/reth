use crate::{
    db::Transaction, error::*, util::opt::MaybeSender, ExecInput, ExecOutput, Stage, StageError,
    StageId, UnwindInput,
};
use reth_db::{database::Database, transaction::DbTx};
use reth_primitives::BlockNumber;
use std::{
    fmt::{Debug, Formatter},
    ops::Deref,
    sync::Arc,
};
use tokio::sync::mpsc::Sender;
use tracing::*;

mod ctrl;
mod event;
mod state;

use ctrl::*;
pub use event::*;
use state::*;

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
// ANCHOR: struct-Pipeline
pub struct Pipeline<DB: Database> {
    stages: Vec<QueuedStage<DB>>,
    max_block: Option<BlockNumber>,
    events_sender: MaybeSender<PipelineEvent>,
}
// ANCHOR_END: struct-Pipeline

impl<DB: Database> Default for Pipeline<DB> {
    fn default() -> Self {
        Self { stages: Vec::new(), max_block: None, events_sender: MaybeSender::new(None) }
    }
}
impl<DB: Database> Debug for Pipeline<DB> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").field("max_block", &self.max_block).finish()
    }
}

impl<DB: Database> Pipeline<DB> {
    /// Create a new pipeline.
    pub fn new() -> Self {
        Default::default()
    }

    /// Create a new pipeline with a channel for receiving events (see [PipelineEvent]).
    pub fn new_with_channel(sender: Sender<PipelineEvent>) -> Self {
        Self::new().set_channel(sender)
    }

    /// Add a stage to the pipeline.
    pub fn push<S>(mut self, stage: S) -> Self
    where
        S: Stage<DB> + 'static,
    {
        self.stages.push(QueuedStage { stage: Box::new(stage) });
        self
    }

    /// Set the target block.
    ///
    /// Once this block is reached, syncing will stop.
    pub fn set_max_block(mut self, block: Option<BlockNumber>) -> Self {
        self.max_block = block;
        self
    }

    /// Set a channel the pipeline will transmit events over (see [PipelineEvent]).
    pub fn set_channel(mut self, sender: Sender<PipelineEvent>) -> Self {
        self.events_sender.set(Some(sender));
        self
    }

    /// Run the pipeline in an infinite loop. Will terminate early if the user has specified
    /// a `max_block` in the pipeline.
    pub async fn run(&mut self, db: Arc<DB>) -> Result<(), PipelineError> {
        loop {
            let mut state = PipelineState {
                events_sender: self.events_sender.clone(),
                max_block: self.max_block,
                maximum_progress: None,
                minimum_progress: None,
            };
            let next_action = self.run_loop(&mut state, db.as_ref()).await?;

            // Terminate the loop early if it's reached the maximum user
            // configured block.
            if matches!(next_action, ControlFlow::Continue) &&
                state
                    .minimum_progress
                    .zip(self.max_block)
                    .map_or(false, |(progress, target)| progress >= target)
            {
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
    async fn run_loop(
        &mut self,
        state: &mut PipelineState,
        db: &DB,
    ) -> Result<ControlFlow, PipelineError> {
        let mut previous_stage = None;
        for (_, queued_stage) in self.stages.iter_mut().enumerate() {
            let stage_id = queued_stage.stage.id();
            trace!(
                target: "sync::pipeline",
                stage = %stage_id,
                "Executing stage"
            );
            let next = queued_stage
                .execute(state, previous_stage, db)
                .instrument(info_span!("execute", stage = %stage_id))
                .await?;

            match next {
                ControlFlow::Continue => {
                    let tx = db.tx()?;
                    previous_stage =
                        Some((stage_id, stage_id.get_progress(&tx)?.unwrap_or_default()));
                    tx.commit()?;
                }
                ControlFlow::Unwind { target, bad_block } => {
                    self.unwind(db, target, bad_block).await?;

                    return Ok(ControlFlow::Unwind { target, bad_block })
                }
            }
        }

        Ok(ControlFlow::Continue)
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

        for QueuedStage { stage, .. } in unwind_pipeline {
            let stage_id = stage.id();
            let span = info_span!("Unwinding", stage = %stage_id);
            let _enter = span.enter();

            let mut stage_progress = stage_id.get_progress(tx.deref())?.unwrap_or_default();
            if stage_progress < to {
                debug!(from = %stage_progress, %to, "Unwind point too far for stage");
                self.events_sender.send(PipelineEvent::Skipped { stage_id }).await?;
                return Ok(())
            }

            debug!(from = %stage_progress, %to, ?bad_block, "Starting unwind");
            while stage_progress > to {
                let input = UnwindInput { stage_progress, unwind_to: to, bad_block };
                self.events_sender.send(PipelineEvent::Unwinding { stage_id, input }).await?;

                let output = stage.unwind(&mut tx, input).await;
                match output {
                    Ok(unwind_output) => {
                        stage_progress = unwind_output.stage_progress;
                        stage_id.save_progress(tx.deref(), stage_progress)?;

                        self.events_sender
                            .send(PipelineEvent::Unwound { stage_id, result: unwind_output })
                            .await?;
                    }
                    Err(err) => {
                        self.events_sender.send(PipelineEvent::Error { stage_id }).await?;
                        return Err(PipelineError::Stage(StageError::Fatal(err)))
                    }
                }
            }
        }

        tx.commit()?;
        Ok(())
    }
}

/// A container for a queued stage.
struct QueuedStage<DB: Database> {
    /// The actual stage to execute.
    stage: Box<dyn Stage<DB>>,
}

impl<DB: Database> QueuedStage<DB> {
    /// Execute the stage.
    async fn execute<'tx>(
        &mut self,
        state: &mut PipelineState,
        previous_stage: Option<(StageId, BlockNumber)>,
        db: &DB,
    ) -> Result<ControlFlow, PipelineError> {
        let stage_id = self.stage.id();
        loop {
            let mut tx = Transaction::new(db)?;

            let prev_progress = stage_id.get_progress(tx.deref())?;

            let stage_reached_max_block = prev_progress
                .zip(state.max_block)
                .map_or(false, |(prev_progress, target)| prev_progress >= target);
            if stage_reached_max_block {
                warn!(
                    target: "sync::pipeline",
                    stage = %stage_id,
                    "Stage reached maximum block, skipping."
                );
                state.events_sender.send(PipelineEvent::Skipped { stage_id }).await?;

                // We reached the maximum block, so we skip the stage
                return Ok(ControlFlow::Continue)
            }

            state
                .events_sender
                .send(PipelineEvent::Running { stage_id, stage_progress: prev_progress })
                .await?;

            match self
                .stage
                .execute(&mut tx, ExecInput { previous_stage, stage_progress: prev_progress })
                .await
            {
                Ok(out @ ExecOutput { stage_progress, done }) => {
                    info!(
                        target: "sync::pipeline",
                        stage = %stage_id,
                        %stage_progress,
                        %done,
                        "Stage made progress"
                    );
                    stage_id.save_progress(tx.deref(), stage_progress)?;

                    state
                        .events_sender
                        .send(PipelineEvent::Ran { stage_id, result: out.clone() })
                        .await?;

                    // TODO: Make the commit interval configurable
                    tx.commit()?;

                    state.record_progress_outliers(stage_progress);

                    if done {
                        return Ok(ControlFlow::Continue)
                    }
                }
                Err(err) => {
                    state.events_sender.send(PipelineEvent::Error { stage_id }).await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StageId, UnwindOutput};
    use assert_matches::assert_matches;
    use reth_db::mdbx::{self, test_utils, Env, EnvKind, WriteMap};
    use reth_interfaces::consensus;
    use tokio::sync::mpsc::channel;
    use tokio_stream::{wrappers::ReceiverStream, StreamExt};
    use utils::TestStage;

    /// Runs a simple pipeline.
    #[tokio::test]
    async fn run_pipeline() {
        let (tx, rx) = channel(2);
        let db = test_utils::create_test_db(EnvKind::RW);

        // Run pipeline
        tokio::spawn(async move {
            Pipeline::<Env<WriteMap>>::new_with_channel(tx)
                .push(
                    TestStage::new(StageId("A"))
                        .add_exec(Ok(ExecOutput { stage_progress: 20, done: true })),
                )
                .push(
                    TestStage::new(StageId("B"))
                        .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
                )
                .set_max_block(Some(10))
                .run(db)
                .await
        });

        // Check that the stages were run in order
        assert_eq!(
            ReceiverStream::new(rx).collect::<Vec<PipelineEvent>>().await,
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
        let (tx, rx) = channel(2);
        let db = test_utils::create_test_db(EnvKind::RW);

        // Run pipeline
        tokio::spawn(async move {
            let mut pipeline = Pipeline::<Env<mdbx::WriteMap>>::new()
                .push(
                    TestStage::new(StageId("A"))
                        .add_exec(Ok(ExecOutput { stage_progress: 100, done: true }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
                )
                .push(
                    TestStage::new(StageId("B"))
                        .add_exec(Ok(ExecOutput { stage_progress: 10, done: true }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
                )
                .push(
                    TestStage::new(StageId("C"))
                        .add_exec(Ok(ExecOutput { stage_progress: 20, done: true }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
                )
                .set_max_block(Some(10));

            // Sync first
            pipeline.run(db.clone()).await.expect("Could not run pipeline");

            // Unwind
            pipeline.set_channel(tx).unwind(&db, 1, None).await.expect("Could not unwind pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            ReceiverStream::new(rx).collect::<Vec<PipelineEvent>>().await,
            vec![
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
        let (tx, rx) = channel(2);
        let db = test_utils::create_test_db(EnvKind::RW);

        // Run pipeline
        tokio::spawn(async move {
            Pipeline::<Env<mdbx::WriteMap>>::new()
                .push(
                    TestStage::new(StageId("A"))
                        .add_exec(Ok(ExecOutput { stage_progress: 10, done: true }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 0 }))
                        .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
                )
                .push(
                    TestStage::new(StageId("B"))
                        .add_exec(Err(StageError::Validation {
                            block: 5,
                            error: consensus::Error::BaseFeeMissing,
                        }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 0 }))
                        .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
                )
                .set_max_block(Some(10))
                .set_channel(tx)
                .run(db)
                .await
                .expect("Could not run pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            ReceiverStream::new(rx).collect::<Vec<PipelineEvent>>().await,
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
        let db = test_utils::create_test_db(EnvKind::RW);
        let result = Pipeline::<Env<WriteMap>>::new()
            .push(
                TestStage::new(StageId("NonFatal"))
                    .add_exec(Err(StageError::Recoverable(Box::new(std::fmt::Error))))
                    .add_exec(Ok(ExecOutput { stage_progress: 10, done: true })),
            )
            .set_max_block(Some(10))
            .run(db)
            .await;
        assert_matches!(result, Ok(()));

        // Fatal
        let db = test_utils::create_test_db(EnvKind::RW);
        let result = Pipeline::<Env<WriteMap>>::new()
            .push(TestStage::new(StageId("Fatal")).add_exec(Err(StageError::DatabaseIntegrity(
                DatabaseIntegrityError::BlockBody { number: 5 },
            ))))
            .run(db)
            .await;
        assert_matches!(
            result,
            Err(PipelineError::Stage(StageError::DatabaseIntegrity(
                DatabaseIntegrityError::BlockBody { number: 5 }
            )))
        );
    }

    mod utils {
        use super::*;
        use async_trait::async_trait;
        use std::{collections::VecDeque, error::Error};

        pub(crate) struct TestStage {
            id: StageId,
            exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
            unwind_outputs: VecDeque<Result<UnwindOutput, Box<dyn Error + Send + Sync>>>,
        }

        impl TestStage {
            pub(crate) fn new(id: StageId) -> Self {
                Self { id, exec_outputs: VecDeque::new(), unwind_outputs: VecDeque::new() }
            }

            pub(crate) fn add_exec(mut self, output: Result<ExecOutput, StageError>) -> Self {
                self.exec_outputs.push_back(output);
                self
            }

            pub(crate) fn add_unwind(
                mut self,
                output: Result<UnwindOutput, Box<dyn Error + Send + Sync>>,
            ) -> Self {
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
            ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
                self.unwind_outputs
                    .pop_front()
                    .unwrap_or_else(|| panic!("Test stage {} unwound too many times.", self.id))
            }
        }
    }
}
