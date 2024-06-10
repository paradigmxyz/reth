use crate::{chain::PipelineAction, engine::DownloadRequest};
use parking_lot::Mutex;
use reth_beacon_consensus::{ForkchoiceStateTracker, InvalidHeaderCache, OnForkChoiceUpdated};
use reth_engine_primitives::EngineTypes;
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::SealedBlockWithSenders;
use reth_rpc_types::{
    engine::{CancunPayloadFields, ForkchoiceState, PayloadStatus},
    ExecutionPayload,
};
use std::{marker::PhantomData, sync::Arc};

/// Keeps track of the state of the tree.
#[derive(Clone, Debug)]
pub struct TreeState {
    // TODO: this is shared state for the blocks etc.
}

impl TreeState {
    fn buffer(&mut self) {}

    fn insert_validated(&mut self) {}
}

/// Tracks the state of the engine api internals.
///
/// This type is shareable.
#[derive(Clone, Debug)]
pub struct EngineApiTreeState {
    /// Tracks the received forkchoice state updates received by the CL.
    forkchoice_state_tracker: ForkchoiceStateTracker,
    /// Tracks the header of invalid payloads that were rejected by the engine because they're
    /// invalid.
    invalid_headers: Arc<Mutex<InvalidHeaderCache>>,

    /// Tracks the state of the blockchain tree
    tree_state: TreeState,
}

/// The type responsible for processing engine API requests.
///
/// TODO: design: should the engine handler functions also accept the response channel or return the
/// result and the caller redirects the response
pub trait EngineApiTreeHandler: Send + Sync + Clone {
    /// The engine type that this handler is for.
    type Engine: EngineTypes;

    /// Invoked when previously requested blocks were downloaded.
    fn on_downloaded(&self, blocks: Vec<SealedBlockWithSenders>) -> Option<TreeEvent>;

    /// When the Consensus layer receives a new block via the consensus gossip protocol,
    /// the transactions in the block are sent to the execution layer in the form of a
    /// [`ExecutionPayload`]. The Execution layer executes the transactions and validates the
    /// state in the block header, then passes validation data back to Consensus layer, that
    /// adds the block to the head of its own blockchain and attests to it. The block is then
    /// broadcast over the consensus p2p network in the form of a "Beacon block".
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_newPayload`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification).
    ///
    /// This returns a [`PayloadStatus`] that represents the outcome of a processed new payload and
    /// returns an error if an internal error occurred.
    fn on_new_payload(
        &self,
        payload: ExecutionPayload,
        cancun_fields: Option<CancunPayloadFields>,
    ) -> TreeOutcome<PayloadStatus>;

    /// Invoked when we receive a new forkchoice update message. Calls into the blockchain tree
    /// to resolve chain forks and ensure that the Execution Layer is working with the latest valid
    /// chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    ///
    /// Returns an error if an internal error occurred like a database error.
    fn on_forkchoice_updated(
        &self,
        state: ForkchoiceState,
        attrs: Option<<Self::Engine as EngineTypes>::PayloadAttributes>,
    ) -> TreeOutcome<Result<OnForkChoiceUpdated, String>>;
}

/// The outcome of a tree operation.
#[derive(Debug)]
pub struct TreeOutcome<T> {
    /// The outcome of the operation.
    pub outcome: T,
    /// An optional event to tell the caller to do something.
    pub event: Option<TreeEvent>,
}

/// Events that can be emitted by the [EngineApiTreeHandler].
#[derive(Debug)]
pub enum TreeEvent {
    PipelineAction(PipelineAction),
    Download(DownloadRequest),
}

#[derive(Clone, Debug)]
pub struct EngineApiTreeHandlerImpl<T: EngineTypes> {
    state: EngineApiTreeState,
    payload_validator: ExecutionPayloadValidator,
    _marker: PhantomData<T>,
}

impl<T: EngineTypes> EngineApiTreeHandler for EngineApiTreeHandlerImpl<T> {
    type Engine = T;

    fn on_downloaded(&self, blocks: Vec<SealedBlockWithSenders>) -> Option<TreeEvent> {
        todo!()
    }

    fn on_new_payload(
        &self,
        payload: ExecutionPayload,
        cancun_fields: Option<CancunPayloadFields>,
    ) -> TreeOutcome<PayloadStatus> {
        todo!()
    }

    fn on_forkchoice_updated(
        &self,
        state: ForkchoiceState,
        attrs: Option<<Self::Engine as EngineTypes>::PayloadAttributes>,
    ) -> TreeOutcome<Result<OnForkChoiceUpdated, String>> {
        todo!()
    }
}
