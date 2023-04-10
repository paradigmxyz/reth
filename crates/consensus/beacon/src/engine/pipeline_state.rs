use reth_db::database::Database;
use reth_interfaces::sync::SyncStateUpdater;
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
pub enum PipelineState<DB: Database, U: SyncStateUpdater> {
    /// Pipeline is idle.
    Idle(Pipeline<DB, U>),
    /// Pipeline is running.
    Running(oneshot::Receiver<PipelineWithResult<DB, U>>),
}

impl<DB: Database, U: SyncStateUpdater> PipelineState<DB, U> {
    /// Returns `true` if the state matches idle.
    pub fn is_idle(&self) -> bool {
        matches!(self, PipelineState::Idle(_))
    }
}
