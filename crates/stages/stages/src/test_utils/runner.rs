use super::TestStageDB;
use reth_chainspec::ChainSpec;
use reth_db::{test_utils::TempDatabase, Database, DatabaseEnv};
use reth_provider::{DatabaseProvider, ProviderError};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageError, StageExt, UnwindInput, UnwindOutput,
};
use reth_storage_errors::db::DatabaseError;
use tokio::sync::oneshot;

#[derive(thiserror::Error, Debug)]
pub(crate) enum TestRunnerError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Internal(#[from] Box<dyn core::error::Error>),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// A generic test runner for stages.
pub(crate) trait StageTestRunner {
    type S: Stage<DatabaseProvider<<TempDatabase<DatabaseEnv> as Database>::TXMut, ChainSpec>>
        + 'static;

    /// Return a reference to the database.
    fn db(&self) -> &TestStageDB;

    /// Return an instance of a Stage.
    fn stage(&self) -> Self::S;
}

pub(crate) trait ExecuteStageTestRunner: StageTestRunner {
    type Seed: Send + Sync;

    /// Seed database for stage execution
    fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError>;

    /// Validate stage execution
    fn validate_execution(
        &self,
        input: ExecInput,
        output: Option<ExecOutput>,
    ) -> Result<(), TestRunnerError>;

    /// Run [`Stage::execute`] and return a receiver for the result.
    fn execute(&self, input: ExecInput) -> oneshot::Receiver<Result<ExecOutput, StageError>> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.db().factory.clone(), self.stage());
        tokio::spawn(async move {
            let result = stage.execute_ready(input).await.and_then(|_| {
                let provider_rw = db.provider_rw().unwrap();
                let result = stage.execute(&provider_rw, input);
                provider_rw.commit().expect("failed to commit");
                result
            });
            tx.send(result).expect("failed to send message")
        });
        rx
    }

    /// Run a hook after [`Stage::execute`]. Required for Headers & Bodies stages.
    async fn after_execution(&self, _seed: Self::Seed) -> Result<(), TestRunnerError> {
        Ok(())
    }
}

pub(crate) trait UnwindStageTestRunner: StageTestRunner {
    /// Validate the unwind
    fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError>;

    /// Run [`Stage::unwind`] and return a receiver for the result.
    async fn unwind(&self, input: UnwindInput) -> Result<UnwindOutput, StageError> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.db().factory.clone(), self.stage());
        tokio::spawn(async move {
            let provider = db.provider_rw().unwrap();
            let result = stage.unwind(&provider, input);
            provider.commit().expect("failed to commit");
            tx.send(result).expect("failed to send result");
        });
        rx.await.unwrap()
    }

    /// Run a hook before [`Stage::unwind`]. Required for `MerkleStage`.
    fn before_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
        Ok(())
    }
}
