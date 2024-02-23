use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    static_file::HighestStaticFiles,
};
use reth_provider::{DatabaseProviderRW, StageCheckpointReader};
use reth_static_file::StaticFileProducer;

/// The static file stage _copies_ all data from database to static files using
/// [StaticFileProducer]. The block range for copying is determined by the current highest blocks
/// contained in static files and stage checkpoints for each segment individually.
#[derive(Debug)]
pub struct StaticFileStage<DB: Database> {
    static_file_producer: StaticFileProducer<DB>,
}

impl<DB: Database> StaticFileStage<DB> {
    /// Creates a new static file stage.
    pub fn new(static_file_producer: StaticFileProducer<DB>) -> Self {
        Self { static_file_producer }
    }
}

impl<DB: Database> Stage<DB> for StaticFileStage<DB> {
    fn id(&self) -> StageId {
        StageId::StaticFile
    }

    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let targets = self.static_file_producer.get_static_file_targets(HighestStaticFiles {
            headers: provider
                .get_stage_checkpoint(StageId::Headers)?
                .map(|checkpoint| checkpoint.block_number),
            receipts: provider
                .get_stage_checkpoint(StageId::Execution)?
                .map(|checkpoint| checkpoint.block_number),
            transactions: provider
                .get_stage_checkpoint(StageId::Bodies)?
                .map(|checkpoint| checkpoint.block_number),
        })?;
        self.static_file_producer.run(targets)?;
        Ok(ExecOutput::done(input.checkpoint()))
    }

    fn unwind(
        &mut self,
        _provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}
