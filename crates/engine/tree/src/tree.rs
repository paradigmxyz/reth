use crate::{chain::PipelineAction, engine::DownloadRequest};
use parking_lot::Mutex;
use reth_beacon_consensus::InvalidHeaderCache;
use reth_engine_primitives::EngineTypes;
use reth_primitives::SealedBlockWithSenders;
use std::sync::Arc;

/// Keeps track of the state of the tree.
pub struct TreeState {
    // TODO: this is shared state
}

impl TreeState {
    fn buffer(&mut self) {}

    fn insert_validated(&mut self) {}
}

/// Tracks the state of the engine api internals.
///
/// This type is shareable.
pub struct EngineApiTreeState {
    /// Tracks the header of invalid payloads that were rejected by the engine because they're
    /// invalid.
    invalid_headers: Arc<Mutex<InvalidHeaderCache>>,
    tree_state: TreeState,
}

/// Access to tree internals.
#[derive(Debug)]
pub struct TreeContext {}

/// The type responsible for processing engine API requests.
pub trait EngineApiTreeHandler: Send + Sync + Clone {
    type Engine: EngineTypes;

    fn downloaded(&mut self, blocks: Vec<SealedBlockWithSenders>) {}

    fn new_payload(&mut self) {}

    fn forkchoice_updated(&mut self) {}
}

/// Events that can be emitted by the [EngineApiTreeHandler].
pub enum TreeEvent {
    PipelineAction(PipelineAction),
    Download(DownloadRequest),
}
