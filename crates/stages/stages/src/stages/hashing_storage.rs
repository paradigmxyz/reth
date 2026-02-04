use reth_config::config::{EtlConfig, HashingConfig};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use std::fmt::Debug;

/// Storage hashing stage.
///
/// This stage is now a no-op because hashing is done during execution - state is written
/// directly to hashed tables (`HashedAccounts`, `HashedStorages`) during block execution.
#[derive(Debug, Default)]
pub struct StorageHashingStage {
    /// The threshold (in number of blocks) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of slots to process before committing during unwind.
    pub commit_threshold: u64,
    /// ETL configuration
    pub etl_config: EtlConfig,
}

impl StorageHashingStage {
    /// Create new instance of [`StorageHashingStage`].
    pub const fn new(config: HashingConfig, etl_config: EtlConfig) -> Self {
        Self {
            clean_threshold: config.clean_threshold,
            commit_threshold: config.commit_threshold,
            etl_config,
        }
    }
}

impl<Provider> Stage<Provider> for StorageHashingStage {
    fn id(&self) -> StageId {
        StageId::StorageHashing
    }

    fn execute(&mut self, _provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        Ok(ExecOutput::done(StageCheckpoint::new(input.target())))
    }

    fn unwind(
        &mut self,
        _provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}
