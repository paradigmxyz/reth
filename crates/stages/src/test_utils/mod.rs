use crate::StageId;

mod macros;
mod runner;
mod stage_db;

pub(crate) use macros::*;
pub(crate) use runner::{
    ExecuteStageTestRunner, StageTestRunner, TestRunnerError, UnwindStageTestRunner,
};
pub(crate) use stage_db::StageTestDB;

/// The previous test stage id mock used for testing
pub(crate) const PREV_STAGE_ID: StageId = StageId("PrevStage");
