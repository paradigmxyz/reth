use crate::{
    error::*,
    util::opt::{self, MaybeSender},
    ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput,
};
use reth_db::mdbx;
use reth_primitives::BlockNumber;
use std::fmt::{Debug, Formatter};
use tokio::sync::mpsc::Sender;
use tracing::*;

mod event;
pub use event::*;

struct QueuedStage<'db, E>
where
    E: mdbx::EnvironmentKind,
{
    /// The actual stage to execute.
    stage: Box<dyn Stage<'db, E>>,
    /// The unwind priority of the stage.
    unwind_priority: usize,
    /// Whether or not this stage can only execute when we reach what we believe to be the tip of
    /// the chain.
    require_tip: bool,
}

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
/// # Unwinding
///
/// In case of a validation error (as determined by the consensus engine) in one of the stages, the
/// pipeline will unwind the stages according to their unwind priority. It is also possible to
/// request an unwind manually (see [Pipeline::unwind]).
///
/// The unwind priority is set with [Pipeline::push_with_unwind_priority]. Stages with higher unwind
/// priorities are unwound first.
pub struct Pipeline<'db, E>
where
    E: mdbx::EnvironmentKind,
{
    stages: Vec<QueuedStage<'db, E>>,
    max_block: Option<BlockNumber>,
    events_sender: MaybeSender<PipelineEvent>,
}

impl<'db, E> Default for Pipeline<'db, E>
where
    E: mdbx::EnvironmentKind,
{
    fn default() -> Self {
        Self { stages: Vec::new(), max_block: None, events_sender: MaybeSender::new(None) }
    }
}

impl<'db, E> Debug for Pipeline<'db, E>
where
    E: mdbx::EnvironmentKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").field("max_block", &self.max_block).finish()
    }
}

impl<'db, E> Pipeline<'db, E>
where
    E: mdbx::EnvironmentKind,
{
    /// Create a new pipeline.
    pub fn new() -> Self {
        Default::default()
    }

    /// Create a new pipeline with a channel for receiving events (see [PipelineEvent]).
    pub fn new_with_channel(sender: Sender<PipelineEvent>) -> Self {
        Self::new().set_channel(sender)
    }

    /// Add a stage to the pipeline.
    ///
    /// # Unwinding
    ///
    /// The unwind priority is set to 0.
    pub fn push<S>(self, stage: S, require_tip: bool) -> Self
    where
        S: Stage<'db, E> + 'static,
    {
        self.push_with_unwind_priority(stage, require_tip, 0)
    }

    /// Add a stage to the pipeline, specifying the unwind priority.
    pub fn push_with_unwind_priority<S>(
        mut self,
        stage: S,
        require_tip: bool,
        unwind_priority: usize,
    ) -> Self
    where
        S: Stage<'db, E> + 'static,
    {
        self.stages.push(QueuedStage { stage: Box::new(stage), require_tip, unwind_priority });
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

    /// Run the pipeline.
    pub async fn run(&mut self, db: &'db mdbx::Environment<E>) -> Result<(), PipelineError> {
        let mut previous_stage = None;
        let mut minimum_progress: Option<BlockNumber> = None;
        let mut maximum_progress: Option<BlockNumber> = None;
        let mut reached_tip_flag = true;

        'run: loop {
            let mut tx = db.begin_rw_txn()?;
            for (_, queued_stage) in self.stages.iter_mut().enumerate() {
                let stage_id = queued_stage.stage.id();
                let block_reached = loop {
                    let prev_progress = stage_id.get_progress(&tx)?;
                    self.events_sender
                        .send(PipelineEvent::Running { stage_id, stage_progress: prev_progress })
                        .await?;

                    // Whether any stage has reached the maximum block, which also counts as having
                    // reached the tip for stages that have reached the tip
                    let reached_max_block = maximum_progress
                        .zip(self.max_block)
                        .map_or(false, |(progress, target)| progress >= target);

                    // Whether this stage reached the max block
                    let stage_reached_max_block = prev_progress
                        .zip(self.max_block)
                        .map_or(false, |(prev_progress, target)| prev_progress >= target);

                    // Execute stage
                    let output = Self::execute_stage(
                        &mut tx,
                        queued_stage,
                        ExecInput { previous_stage, stage_progress: prev_progress },
                        reached_tip_flag || reached_max_block,
                        stage_reached_max_block,
                    )
                    .instrument(info_span!("Running", stage = %stage_id))
                    .await;

                    if output.is_err() {
                        self.events_sender
                            .send(PipelineEvent::Ran { stage_id, result: None })
                            .await?;
                    }

                    match output {
                        Ok(out @ ExecOutput { stage_progress, done, reached_tip }) => {
                            debug!(stage = %stage_id, %stage_progress, %done, "Stage made progress");
                            stage_id.save_progress(&tx, stage_progress)?;

                            self.events_sender
                                .send(PipelineEvent::Ran { stage_id, result: Some(out.clone()) })
                                .await?;

                            // TODO: Make the commit interval configurable
                            tx.commit()?;
                            tx = db.begin_rw_txn()?;

                            // Update our minimum and maximum stage progress
                            minimum_progress = opt::min(minimum_progress, stage_progress);
                            maximum_progress = opt::max(maximum_progress, stage_progress);

                            if done {
                                reached_tip_flag = reached_tip;
                                break stage_progress
                            }
                        }
                        Err(StageError::Validation { block }) => {
                            debug!(stage = %stage_id, bad_block = %block, "Stage encountered a validation error.");

                            // We unwind because of a validation error. If the unwind itself fails,
                            // we bail entirely, otherwise we restart the execution loop from the
                            // beginning.
                            //
                            // Note on the drop: The transaction needs to be dropped in order for
                            // unwind to create a new one. Dropping the
                            // transaction will abort it; there is no
                            // other way currently to abort the transaction. It will be re-created
                            // if the loop restarts.
                            drop(tx);
                            match self
                                .unwind(db, prev_progress.unwrap_or_default(), Some(block))
                                .await
                            {
                                Ok(()) => continue 'run,
                                Err(e) => return Err(e),
                            }
                        }
                        Err(e) => return Err(PipelineError::Stage(e)),
                    }
                };

                // Set previous stage and continue on to next stage.
                previous_stage = Some((stage_id, block_reached));
            }
            tx.commit()?;

            // Check if we've reached our desired target block
            if minimum_progress
                .zip(self.max_block)
                .map_or(false, |(progress, target)| progress >= target)
            {
                return Ok(())
            }
        }
    }

    /// Unwind the stages to the target block.
    ///
    /// If the unwind is due to a bad block the number of that block should be specified.
    pub async fn unwind(
        &mut self,
        db: &'db mdbx::Environment<E>,
        to: BlockNumber,
        bad_block: Option<BlockNumber>,
    ) -> Result<(), PipelineError> {
        // Sort stages by unwind priority
        let mut unwind_pipeline = {
            let mut stages: Vec<_> = self.stages.iter_mut().enumerate().collect();
            stages.sort_by_key(|(id, stage)| {
                if stage.unwind_priority > 0 {
                    (id - stage.unwind_priority, 0)
                } else {
                    (*id, 1)
                }
            });
            stages.reverse();
            stages
        };

        // Unwind stages in reverse order of priority (i.e. higher priority = first)
        let mut tx = db.begin_rw_txn()?;
        for (_, QueuedStage { stage, .. }) in unwind_pipeline.iter_mut() {
            let stage_id = stage.id();
            let mut stage_progress = stage_id.get_progress(&tx)?.unwrap_or_default();

            let unwind: Result<(), PipelineError> = async {
                if stage_progress < to {
                    debug!(from = %stage_progress, %to, "Unwind point too far for stage");
                    self.events_sender
                        .send(PipelineEvent::Unwound {
                            stage_id,
                            result: Some(UnwindOutput { stage_progress }),
                        })
                        .await?;
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
                            stage_id.save_progress(&tx, stage_progress)?;

                            self.events_sender
                                .send(PipelineEvent::Unwound {
                                    stage_id,
                                    result: Some(unwind_output),
                                })
                                .await?;
                        }
                        Err(err) => {
                            self.events_sender
                                .send(PipelineEvent::Unwound { stage_id, result: None })
                                .await?;
                            return Err(PipelineError::Stage(StageError::Internal(err)))
                        }
                    }
                }

                Ok(())
            }
            .instrument(info_span!("Unwinding", stage = %stage_id))
            .await;
            unwind?
        }

        tx.commit()?;
        Ok(())
    }
}

impl<'db, E> Pipeline<'db, E>
where
    E: mdbx::EnvironmentKind,
{
    async fn execute_stage<'tx>(
        tx: &mut mdbx::Transaction<'tx, mdbx::RW, E>,
        QueuedStage { stage, require_tip, .. }: &mut QueuedStage<'db, E>,
        input: ExecInput,
        reached_tip: bool,
        stage_reached_max_block: bool,
    ) -> Result<ExecOutput, StageError>
    where
        'db: 'tx,
    {
        if !reached_tip && *require_tip {
            info!("Tip not reached, skipping.");

            // Stage requires us to reach the tip of the chain first, but we have
            // not.
            Ok(ExecOutput {
                stage_progress: input.stage_progress.unwrap_or_default(),
                done: true,
                reached_tip: false,
            })
        } else if stage_reached_max_block {
            info!("Stage reached maximum block, skipping.");
            // We reached the maximum block, so we skip the stage
            Ok(ExecOutput {
                stage_progress: input.stage_progress.unwrap_or_default(),
                done: true,
                reached_tip: true,
            })
        } else {
            stage.execute(tx, input).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StageId, UnwindOutput};
    use reth_db::mdbx;
    use tempfile::tempdir;
    use tokio::sync::mpsc::channel;
    use tokio_stream::{wrappers::ReceiverStream, StreamExt};
    use utils::TestStage;

    /// Runs a simple pipeline.
    #[tokio::test]
    async fn run_pipeline() {
        let (tx, rx) = channel(2);
        let db = utils::test_db().expect("Could not open test database");

        // Run pipeline
        tokio::spawn(async move {
            Pipeline::<mdbx::WriteMap>::new_with_channel(tx)
                .push(
                    TestStage::new(StageId("A")).add_exec(Ok(ExecOutput {
                        stage_progress: 20,
                        done: true,
                        reached_tip: true,
                    })),
                    false,
                )
                .push(
                    TestStage::new(StageId("B")).add_exec(Ok(ExecOutput {
                        stage_progress: 10,
                        done: true,
                        reached_tip: true,
                    })),
                    false,
                )
                .set_max_block(Some(10))
                .run(&db)
                .await
        });

        // Check that the stages were run in order
        assert_eq!(
            ReceiverStream::new(rx).collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Running { stage_id: StageId("A"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("A"),
                    result: Some(ExecOutput { stage_progress: 20, done: true, reached_tip: true }),
                },
                PipelineEvent::Running { stage_id: StageId("B"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("B"),
                    result: Some(ExecOutput { stage_progress: 10, done: true, reached_tip: true }),
                },
            ]
        );
    }

    /// Unwinds a simple pipeline.
    #[tokio::test]
    async fn unwind_pipeline() {
        let (tx, rx) = channel(2);
        let db = utils::test_db().expect("Could not open test database");

        // Run pipeline
        tokio::spawn(async move {
            let mut pipeline = Pipeline::<mdbx::WriteMap>::new()
                .push(
                    TestStage::new(StageId("A"))
                        .add_exec(Ok(ExecOutput {
                            stage_progress: 100,
                            done: true,
                            reached_tip: true,
                        }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
                    false,
                )
                .push(
                    TestStage::new(StageId("B"))
                        .add_exec(Ok(ExecOutput {
                            stage_progress: 10,
                            done: true,
                            reached_tip: true,
                        }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 1 })),
                    false,
                )
                .set_max_block(Some(10));

            // Sync first
            pipeline.run(&db).await.expect("Could not run pipeline");

            // Unwind
            pipeline.set_channel(tx).unwind(&db, 1, None).await.expect("Could not unwind pipeline");
        });

        // Check that the stages were unwound in reverse order
        assert_eq!(
            ReceiverStream::new(rx).collect::<Vec<PipelineEvent>>().await,
            vec![
                PipelineEvent::Unwinding {
                    stage_id: StageId("B"),
                    input: UnwindInput { stage_progress: 10, unwind_to: 1, bad_block: None }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId("B"),
                    result: Some(UnwindOutput { stage_progress: 1 }),
                },
                PipelineEvent::Unwinding {
                    stage_id: StageId("A"),
                    input: UnwindInput { stage_progress: 100, unwind_to: 1, bad_block: None }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId("A"),
                    result: Some(UnwindOutput { stage_progress: 1 }),
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
        let db = utils::test_db().expect("Could not open test database");

        // Run pipeline
        tokio::spawn(async move {
            Pipeline::<mdbx::WriteMap>::new()
                .push(
                    TestStage::new(StageId("A"))
                        .add_exec(Ok(ExecOutput {
                            stage_progress: 10,
                            done: true,
                            reached_tip: true,
                        }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 0 }))
                        .add_exec(Ok(ExecOutput {
                            stage_progress: 10,
                            done: true,
                            reached_tip: true,
                        })),
                    false,
                )
                .push(
                    TestStage::new(StageId("B"))
                        .add_exec(Err(StageError::Validation { block: 5 }))
                        .add_unwind(Ok(UnwindOutput { stage_progress: 0 }))
                        .add_exec(Ok(ExecOutput {
                            stage_progress: 10,
                            done: true,
                            reached_tip: true,
                        })),
                    false,
                )
                .set_max_block(Some(10))
                .set_channel(tx)
                .run(&db)
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
                    result: Some(ExecOutput { stage_progress: 10, done: true, reached_tip: true }),
                },
                PipelineEvent::Running { stage_id: StageId("B"), stage_progress: None },
                PipelineEvent::Ran { stage_id: StageId("B"), result: None },
                PipelineEvent::Unwinding {
                    stage_id: StageId("A"),
                    input: UnwindInput { stage_progress: 10, unwind_to: 0, bad_block: Some(5) }
                },
                PipelineEvent::Unwound {
                    stage_id: StageId("A"),
                    result: Some(UnwindOutput { stage_progress: 0 }),
                },
                PipelineEvent::Running { stage_id: StageId("A"), stage_progress: Some(0) },
                PipelineEvent::Ran {
                    stage_id: StageId("A"),
                    result: Some(ExecOutput { stage_progress: 10, done: true, reached_tip: true }),
                },
                PipelineEvent::Running { stage_id: StageId("B"), stage_progress: None },
                PipelineEvent::Ran {
                    stage_id: StageId("B"),
                    result: Some(ExecOutput { stage_progress: 10, done: true, reached_tip: true }),
                },
            ]
        );
    }

    mod utils {
        use super::*;
        use async_trait::async_trait;
        use std::{collections::VecDeque, error::Error};

        // TODO: This is... not great.
        pub(crate) fn test_db() -> Result<mdbx::Environment<mdbx::WriteMap>, mdbx::Error> {
            const DB_TABLES: usize = 10;

            // Build environment
            let mut builder = mdbx::Environment::<mdbx::WriteMap>::new();
            builder.set_max_dbs(DB_TABLES);
            builder.set_geometry(mdbx::Geometry {
                size: Some(0..usize::MAX),
                growth_step: None,
                shrink_threshold: None,
                page_size: None,
            });
            builder.set_rp_augment_limit(16 * 256 * 1024);

            // Open
            let tempdir = tempdir().unwrap();
            let path = tempdir.path();
            std::fs::DirBuilder::new().recursive(true).create(path).unwrap();
            let db = builder.open(path)?;

            // Create tables
            let tx = db.begin_rw_txn()?;
            tx.create_db(Some("SyncStage"), mdbx::DatabaseFlags::default())?;
            tx.commit()?;

            Ok(db)
        }

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
        impl<'db, E> Stage<'db, E> for TestStage
        where
            E: mdbx::EnvironmentKind,
        {
            fn id(&self) -> StageId {
                self.id
            }

            async fn execute<'tx>(
                &mut self,
                _: &mut mdbx::Transaction<'tx, mdbx::RW, E>,
                _: ExecInput,
            ) -> Result<ExecOutput, StageError>
            where
                'db: 'tx,
            {
                self.exec_outputs
                    .pop_front()
                    .unwrap_or_else(|| panic!("Test stage {} executed too many times.", self.id))
            }

            async fn unwind<'tx>(
                &mut self,
                _: &mut mdbx::Transaction<'tx, mdbx::RW, E>,
                _: UnwindInput,
            ) -> Result<UnwindOutput, Box<dyn Error + Send + Sync>>
            where
                'db: 'tx,
            {
                self.unwind_outputs
                    .pop_front()
                    .unwrap_or_else(|| panic!("Test stage {} unwound too many times.", self.id))
            }
        }
    }
}
