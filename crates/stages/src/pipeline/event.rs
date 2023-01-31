use crate::{
    id::StageId,
    stage::{ExecOutput, UnwindInput, UnwindOutput},
};
use reth_primitives::BlockNumber;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// An event emitted by a [Pipeline][crate::Pipeline].
///
/// It is possible for multiple of these events to be emitted over the duration of a pipeline's
/// execution since:
///
/// - Other stages may ask the pipeline to unwind
/// - The pipeline will loop indefinitely unless a target block is set
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PipelineEvent {
    /// Emitted when a stage is about to be run.
    Running {
        /// The stage that is about to be run.
        stage_id: StageId,
        /// The previous checkpoint of the stage.
        stage_progress: Option<BlockNumber>,
    },
    /// Emitted when a stage has run a single time.
    Ran {
        /// The stage that was run.
        stage_id: StageId,
        /// The result of executing the stage.
        result: ExecOutput,
    },
    /// Emitted when a stage is about to be unwound.
    Unwinding {
        /// The stage that is about to be unwound.
        stage_id: StageId,
        /// The unwind parameters.
        input: UnwindInput,
    },
    /// Emitted when a stage has been unwound.
    Unwound {
        /// The stage that was unwound.
        stage_id: StageId,
        /// The result of unwinding the stage.
        result: UnwindOutput,
    },
    /// Emitted when a stage encounters an error either during execution or unwinding.
    Error {
        /// The stage that encountered an error.
        stage_id: StageId,
    },
    /// Emitted when a stage was skipped due to it's run conditions not being met:
    ///
    /// - The stage might have progressed beyond the point of our target block
    /// - The stage might not need to be unwound since it has not progressed past the unwind target
    /// - The stage requires that the pipeline has reached the tip, but it has not done so yet
    Skipped {
        /// The stage that was skipped.
        stage_id: StageId,
    },
}

/// Bundles all listeners for [`PipelineEvent`]s
// TODO: Make this a generic utility since the same struct exists in `reth/crates/net/network/src/manager.rs` and sort of in `https://github.com/paradigmxyz/reth/blob/01cb6c07df3205ee2bb55853d39302a7dfefc912/crates/net/discv4/src/lib.rs#L662-L671`
#[derive(Default, Clone, Debug)]
pub(crate) struct PipelineEventListeners {
    /// All listeners for events
    listeners: Vec<mpsc::UnboundedSender<PipelineEvent>>,
}

impl PipelineEventListeners {
    /// Send an event to all listeners.
    ///
    /// Channels that were closed are removed.
    pub(crate) fn notify(&mut self, event: PipelineEvent) {
        self.listeners.retain(|listener| listener.send(event.clone()).is_ok())
    }

    /// Add a new event listener.
    pub(crate) fn new_listener(&mut self) -> UnboundedReceiverStream<PipelineEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.listeners.push(sender);
        UnboundedReceiverStream::new(receiver)
    }
}
