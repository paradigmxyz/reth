//! It is expected that the node has two sync modes:
//!
//!  - Backfill sync: Sync to a certain block height in stages, e.g. download data from p2p then
//!    execute that range.
//!  - Live sync: In this mode the nodes is keeping up with the latest tip and listens for new
//!    requests from the consensus client.
//!
//! These modes are mutually exclusive and the node can only be in one mode at a time.

use reth_stages_api::{ControlFlow, PipelineError, PipelineTarget};
use std::task::{Context, Poll};

/// Backfill sync mode functionality.
pub trait BackfillSync: Send + Sync {
    /// Performs a backfill action.
    fn on_action(&mut self, event: BackfillAction);

    /// Polls the pipeline for completion.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent>;
}

/// The backfill actions that can be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillAction {
    /// Start backfilling with the given target.
    Start(PipelineTarget),
}

/// The events that can be emitted on backfill sync.
#[derive(Debug)]
pub enum BackfillEvent {
    Idle,
    /// Backfill sync started.
    Started(PipelineTarget),
    /// Pipeline finished
    ///
    /// If this is returned, the pipeline is idle.
    Finished(Result<ControlFlow, PipelineError>),
}
