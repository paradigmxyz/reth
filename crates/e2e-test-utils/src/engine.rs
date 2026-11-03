//! Version-agnostic access to the consensus engine of a test node.

use alloy_primitives::Bytes;
use alloy_rpc_types_engine::{ForkchoiceState, ForkchoiceUpdated, PayloadStatus};
use reth_engine_primitives::{
    BeaconForkChoiceUpdateError, BeaconOnNewPayloadError, ConsensusEngineHandle,
};
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};

/// Sends `newPayload` and `forkchoiceUpdated` messages to the consensus engine of a test node like
/// a consensus client, without picking an Engine API version for the active fork.
///
/// The messages go to the engine in-process and skip the Engine API RPC layer. That layer decodes
/// the JSON or SSZ request, rejects a method version that does not match the fork of the payload
/// or payload attributes timestamp, checks that the fields of that fork are present, and maps the
/// errors of the engine to RPC error codes. None of this happens here, so tests of that behaviour
/// must call the versioned RPC methods, e.g. with [`EngineApiClient`] on the client of
/// [`NodeTestContext::auth_server_handle`].
///
/// [`EngineApiClient`]: reth_rpc_api::EngineApiClient
/// [`NodeTestContext::auth_server_handle`]: crate::node::NodeTestContext::auth_server_handle
#[derive(Debug, Clone)]
pub struct EngineTestContext<T: PayloadTypes> {
    handle: ConsensusEngineHandle<T>,
}

impl<T: PayloadTypes> EngineTestContext<T> {
    /// Creates a new engine context that sends its messages to the given engine handle.
    pub const fn new(handle: ConsensusEngineHandle<T>) -> Self {
        Self { handle }
    }

    /// Sends a payload to the engine like `engine_newPayload` and returns the payload status.
    ///
    /// The engine inserts the block without making it canonical, see [`Self::forkchoice_updated`].
    /// An invalid block is reported as an `INVALID` status, not as an error. A built payload of the
    /// node converts into the payload type of the engine.
    ///
    /// Returns an error if the engine can not process the payload at all, e.g. because its
    /// parameters are malformed, which the Engine API returns as an invalid params error.
    pub async fn new_payload(
        &self,
        payload: impl Into<T::ExecutionData>,
    ) -> Result<PayloadStatus, BeaconOnNewPayloadError> {
        self.handle.new_payload(payload.into()).await
    }

    /// Sends the payload of a sealed block to the engine, see [`Self::new_payload`].
    ///
    /// `block_access_list` is the encoded block access list of the block, which payloads carry
    /// from Amsterdam on.
    pub async fn new_payload_from_block(
        &self,
        block: SealedBlock<
            <<T::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
        block_access_list: Option<Bytes>,
    ) -> Result<PayloadStatus, BeaconOnNewPayloadError> {
        self.new_payload(T::block_to_payload(block, block_access_list)).await
    }

    /// Sends a forkchoice update without payload attributes to the engine like
    /// `engine_forkchoiceUpdated` and returns the response.
    ///
    /// The payload status of the response is the outcome for the head block: `VALID` once it is
    /// canonical, `SYNCING` if the engine does not know it or is backfilling, and `INVALID` if it
    /// is or descends from an invalid block.
    ///
    /// Returns an error if the engine rejects the forkchoice state, e.g.
    /// [`ForkchoiceUpdateError::InvalidState`] for a zero head hash, or a safe or finalized block
    /// that is unknown or not on the chain of the head. The Engine API returns that error as code
    /// -38002.
    ///
    /// [`ForkchoiceUpdateError::InvalidState`]: alloy_rpc_types_engine::ForkchoiceUpdateError::InvalidState
    pub async fn forkchoice_updated(
        &self,
        state: ForkchoiceState,
    ) -> Result<ForkchoiceUpdated, BeaconForkChoiceUpdateError> {
        self.handle.fork_choice_updated(state, None).await
    }

    /// Sends a forkchoice update with payload attributes to the engine like
    /// `engine_forkchoiceUpdated`, which starts a payload job on top of the head, and returns the
    /// response with the id of the job.
    ///
    /// The forkchoice state is handled like in [`Self::forkchoice_updated`]. The response has no
    /// payload id if the engine starts no payload job, e.g. because it is syncing.
    ///
    /// Returns [`ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes`] if the attributes are
    /// invalid for the head, e.g. because their timestamp is not greater than the timestamp of the
    /// head. The engine still applies the forkchoice state in that case. The Engine API returns
    /// that error as code -38003.
    ///
    /// [`ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes`]: alloy_rpc_types_engine::ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes
    pub async fn forkchoice_updated_with_attributes(
        &self,
        state: ForkchoiceState,
        attributes: impl Into<T::PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated, BeaconForkChoiceUpdateError> {
        self.handle.fork_choice_updated(state, Some(attributes.into())).await
    }
}
