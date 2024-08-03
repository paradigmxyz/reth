use reth_stages_api::StageId;

#[cfg(test)]
mod macros;
#[cfg(test)]
pub(crate) use macros::*;

#[cfg(test)]
mod runner;
#[cfg(test)]
pub(crate) use runner::{
    ExecuteStageTestRunner, StageTestRunner, TestRunnerError, UnwindStageTestRunner,
};

mod test_db;
pub use test_db::{StorageKind, TestStageDB};

mod set;
pub use set::TestStages;

/// The test stage id
pub const TEST_STAGE_ID: StageId = StageId::Other("TestStage");
