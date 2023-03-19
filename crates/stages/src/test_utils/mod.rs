#![allow(unused)]
use crate::StageId;

mod macros;
pub(crate) use macros::*;

mod runner;
pub(crate) use runner::{
    ExecuteStageTestRunner, StageTestRunner, TestRunnerError, UnwindStageTestRunner,
};

mod test_db;
pub use test_db::TestTransaction;

mod stage;
pub use stage::TestStage;

mod set;
pub use set::TestStages;

/// The test stage id
pub const TEST_STAGE_ID: StageId = StageId("TestStage");

/// The previous test stage id mock used for testing
pub(crate) const PREV_STAGE_ID: StageId = StageId("PrevStage");
