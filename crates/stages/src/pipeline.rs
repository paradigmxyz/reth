use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput};
use reth_db::mdbx;
use reth_primitives::U64;
use std::fmt::{Debug, Formatter};

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
    max_block: Option<U64>,
}

impl<'db, E> Default for Pipeline<'db, E>
where
    E: mdbx::EnvironmentKind,
{
    fn default() -> Self {
        Self { stages: Vec::new(), max_block: None }
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
    /// Add a stage to the pipeline.
    ///
    /// # Unwinding
    ///
    /// The unwind priority is set to 0.
    pub fn push<S>(&mut self, stage: S, require_tip: bool) -> &mut Self
    where
        S: Stage<'db, E> + 'static,
    {
        self.push_with_unwind_priority(stage, require_tip, 0)
    }

    /// Add a stage to the pipeline, specifying the unwind priority.
    pub fn push_with_unwind_priority<S>(
        &mut self,
        stage: S,
        require_tip: bool,
        unwind_priority: usize,
    ) -> &mut Self
    where
        S: Stage<'db, E> + 'static,
    {
        self.stages.push(QueuedStage { stage: Box::new(stage), require_tip, unwind_priority });
        self
    }

    /// Set the target block.
    ///
    /// Once this block is reached, syncing will stop.
    pub fn set_max_block(&mut self, block: Option<U64>) -> &mut Self {
        self.max_block = block;
        self
    }

    /// Run the pipeline.
    pub async fn run(
        &mut self,
        db: &'db mdbx::Environment<E>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut previous_stage = None;
        let mut minimum_progress: Option<U64> = None;
        let mut maximum_progress: Option<U64> = None;
        let mut reached_tip_flag = true;

        'run: loop {
            let mut tx = db.begin_rw_txn()?;
            for (_, QueuedStage { stage, require_tip, .. }) in self.stages.iter_mut().enumerate() {
                let stage_id = stage.id();
                let block_reached = loop {
                    let prev_progress = stage_id.get_progress(&tx)?;
                    let reached_virtual_tip = maximum_progress
                        .zip(self.max_block)
                        .map_or(false, |(progress, target)| progress >= target);

                    // Execute stage
                    let output = if !reached_tip_flag && *require_tip && !reached_virtual_tip {
                        // Stage requires us to reach the tip of the chain first, but we have not.
                        Ok(ExecOutput {
                            stage_progress: prev_progress.unwrap_or_default(),
                            done: true,
                            reached_tip: false,
                        })
                    } else if prev_progress
                        .zip(self.max_block)
                        .map_or(false, |(prev_progress, target)| prev_progress >= target)
                    {
                        // We reached the maximum block, so we skip the stage
                        Ok(ExecOutput {
                            stage_progress: prev_progress.unwrap_or_default(),
                            done: true,
                            reached_tip: true,
                        })
                    } else {
                        stage
                            .execute(
                                &mut tx,
                                ExecInput { previous_stage, stage_progress: prev_progress },
                            )
                            .await
                    };

                    match output {
                        Ok(ExecOutput { stage_progress, done, reached_tip }) => {
                            stage_id.save_progress(&tx, stage_progress)?;

                            // TODO: Make the commit interval configurable
                            tx.commit()?;
                            tx = db.begin_rw_txn()?;

                            // TODO: Clean up
                            if let Some(min) = &mut minimum_progress {
                                *min = std::cmp::min(*min, stage_progress);
                            } else {
                                minimum_progress = Some(stage_progress);
                            }
                            if let Some(max) = &mut maximum_progress {
                                *max = std::cmp::max(*max, stage_progress);
                            } else {
                                maximum_progress = Some(stage_progress);
                            }

                            if done {
                                reached_tip_flag = reached_tip;
                                break stage_progress
                            }
                        }
                        Err(StageError::Validation { block }) => {
                            // We unwind because of a validation error. If the unwind itself fails,
                            // we bail entirely, otherwise we restart the execution loop from the
                            // beginning.
                            match self
                                .unwind(db, prev_progress.unwrap_or_default(), Some(block))
                                .await
                            {
                                Ok(()) => continue 'run,
                                Err(e) => return Err(e),
                            }
                        }
                        Err(StageError::Internal(e)) => return Err(e),
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
        to: U64,
        bad_block: Option<U64>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tx = db.begin_rw_txn()?;
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

        for (_, QueuedStage { stage, .. }) in unwind_pipeline.iter_mut() {
            let stage_id = stage.id();
            let mut stage_progress = stage_id.get_progress(&tx)?.unwrap_or_default();
            while stage_progress > to {
                let unwind_output = stage
                    .unwind(&mut tx, UnwindInput { stage_progress, unwind_to: to, bad_block })
                    .await?;
                stage_progress = unwind_output.stage_progress;
                stage_id.save_progress(&tx, stage_progress)?;
            }
        }

        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StageId, UnwindOutput};
    use async_trait::async_trait;
    use reth_db::mdbx;
    use std::error::Error;
    use tempfile::tempdir;

    struct A;

    #[async_trait]
    impl<'db, E> Stage<'db, E> for A
    where
        E: mdbx::EnvironmentKind,
    {
        fn id(&self) -> StageId {
            StageId("A")
        }

        async fn execute<'tx>(
            &mut self,
            _: &mut mdbx::Transaction<'tx, mdbx::RW, E>,
            _: ExecInput,
        ) -> Result<ExecOutput, StageError>
        where
            'db: 'tx,
        {
            Ok(ExecOutput { stage_progress: 10.into(), done: true, reached_tip: true })
        }

        async fn unwind<'tx>(
            &mut self,
            _: &mut mdbx::Transaction<'tx, mdbx::RW, E>,
            _: UnwindInput,
        ) -> Result<UnwindOutput, Box<dyn Error + Send + Sync>>
        where
            'db: 'tx,
        {
            Ok(UnwindOutput { stage_progress: Default::default() })
        }
    }

    // TODO: This is... not great.
    fn test_db() -> Result<mdbx::Environment<mdbx::WriteMap>, mdbx::Error> {
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

    #[tokio::test]
    async fn run_pipeline() -> Result<(), Box<dyn Error + Send + Sync>> {
        let db = test_db()?;

        Pipeline::<mdbx::WriteMap>::default()
            .push(A, false)
            .set_max_block(Some(10.into()))
            .run(&db)
            .await?;

        Ok(())
    }

    #[tokio::test]
    async fn unwind_pipeline() {}

    #[tokio::test]
    async fn run_pipeline_with_unwind() {}
}
