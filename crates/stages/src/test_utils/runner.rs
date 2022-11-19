use reth_db::{kv::Env, mdbx::WriteMap};
use reth_interfaces::db::{self, DBContainer};
use std::borrow::Borrow;
use tokio::sync::oneshot;

use super::StageTestDB;
use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};

#[derive(thiserror::Error, Debug)]
pub(crate) enum TestRunnerError {
    #[error("Database error occured.")]
    Database(#[from] db::Error),
    #[error("Internal runner error occured.")]
    Internal(#[from] Box<dyn std::error::Error>),
}

/// A generic test runner for stages.
#[async_trait::async_trait]
pub(crate) trait StageTestRunner {
    type S: Stage<Env<WriteMap>> + 'static;

    /// Return a reference to the database.
    fn db(&self) -> &StageTestDB;

    /// Return an instance of a Stage.
    fn stage(&self) -> Self::S;
}

#[async_trait::async_trait]
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

    /// Run [Stage::execute] and return a receiver for the result.
    fn execute(&self, input: ExecInput) -> oneshot::Receiver<Result<ExecOutput, StageError>> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.db().inner(), self.stage());
        tokio::spawn(async move {
            let mut db = DBContainer::new(db.borrow()).expect("failed to create db container");
            let result = stage.execute(&mut db, input).await;
            db.commit().expect("failed to commit");
            tx.send(result).expect("failed to send message")
        });
        rx
    }

    /// Run a hook after [Stage::execute]. Required for Headers & Bodies stages.
    async fn after_execution(&self, _seed: Self::Seed) -> Result<(), TestRunnerError> {
        Ok(())
    }
}

#[async_trait::async_trait]
pub(crate) trait UnwindStageTestRunner: StageTestRunner {
    /// Validate the unwind
    fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError>;

    /// Run [Stage::unwind] and return a receiver for the result.
    async fn unwind(
        &self,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.db().inner(), self.stage());
        tokio::spawn(async move {
            let mut db = DBContainer::new(db.borrow()).expect("failed to create db container");
            let result = stage.unwind(&mut db, input).await;
            db.commit().expect("failed to commit");
            tx.send(result).expect("failed to send result");
        });
        Box::pin(rx).await.unwrap()
    }
}
