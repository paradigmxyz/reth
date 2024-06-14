//! It is expected that the node has two sync modes:
//!
//!  - Pipeline sync: Sync to a certain block height in stages, e.g. download data from p2p then
//!    execute that range.
//!  - Live sync: In this mode the nodes is keeping up with the latest tip and listens for new
//!    requests from the consensus client.
//!
//! These modes are mutually exclusive and the node can only be in one mode at a time.

use reth_stages_api::{ControlFlow, PipelineError, PipelineTarget};
use std::task::{Context, Poll};

/// A handler for the pipeline.
pub trait PipelineHandler: Send + Sync {
    /// Performs an action on the pipeline.
    fn on_action(&mut self, event: PipelineAction);

    /// Polls the pipeline for completion.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<PipelineEvent>;
}

/// The actions that can be performed on the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineAction {
    /// Start the pipeline with the given target.
    Start(PipelineTarget),
}

/// The events that can be emitted by the pipeline.
#[derive(Debug)]
pub enum PipelineEvent {
    Idle,
    /// Pipeline started syncing
    Started(PipelineTarget),
    /// Pipeline finished
    ///
    /// If this is returned, the pipeline is idle.
    Finished(Result<ControlFlow, PipelineError>),
}
