use reth_db::database::Database;
use reth_stages::{Pipeline, PipelineWithResult};
use tokio::sync::oneshot;

/// The possible pipeline states within the sync controller.
///
/// [PipelineState::Idle] means that the pipeline is currently idle.
/// [PipelineState::Running] means that the pipeline is currently running.
///
/// NOTE: The differentiation between these two states is important, because when the pipeline is
/// running, it acquires the write lock over the database. This means that we cannot forward to the
/// blockchain tree any messages that would result in database writes, since it would result in a
/// deadlock.
pub enum PipelineState<DB: Database> {
    /// Pipeline is idle.
    Idle(Pipeline<DB>),
    /// Pipeline is running.
    Running(oneshot::Receiver<PipelineWithResult<DB>>),
}

impl<DB: Database> PipelineState<DB> {
    /// Returns `true` if the state matches idle.
    pub fn is_idle(&self) -> bool {
        matches!(self, PipelineState::Idle(_))
    }
}
