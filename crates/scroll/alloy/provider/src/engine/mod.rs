mod provider;

pub use provider::ScrollAuthEngineApiProvider;

mod client;
pub use client::ScrollAuthApiEngineClient;

use super::error::ScrollEngineApiError;
use alloy_primitives::{BlockHash, U64};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionPayloadBodiesV1, ExecutionPayloadV1, ForkchoiceState,
    ForkchoiceUpdated, PayloadId, PayloadStatus,
};

use scroll_alloy_rpc_types_engine::ScrollPayloadAttributes;

/// A type alias for the result of the Scroll Engine API methods.
pub type ScrollEngineApiResult<T> = Result<T, ScrollEngineApiError>;

/// Engine API trait for Scroll. Only exposes versions of the API that are supported.
/// Note:
/// > The provider should use a JWT authentication layer.
#[async_trait::async_trait]
pub trait ScrollEngineApi {
    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(
        &self,
        payload: ExecutionPayloadV1,
    ) -> ScrollEngineApiResult<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_forkchoiceupdatedv1>
    /// Caution: This should not accept the `withdrawals` field in the payload attributes.
    ///
    /// Modifications:
    /// - Adds the below fields to the `payload_attributes`:
    ///     - transactions: an optional list of transactions to include at the start of the block.
    ///     - `no_tx_pool`: a boolean which signals whether pool transactions need to be included in
    ///       the payload building task.
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<ScrollPayloadAttributes>,
    ) -> ScrollEngineApiResult<ForkchoiceUpdated>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_getpayloadv1>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// Caution: This should not return the `withdrawals` field
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> ScrollEngineApiResult<ExecutionPayloadV1>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> ScrollEngineApiResult<ExecutionPayloadBodiesV1>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyrangev1>
    ///
    /// Returns the execution payload bodies by the range starting at `start`, containing `count`
    /// blocks.
    ///
    /// WARNING: This method is associated with the `BeaconBlocksByRange` message in the consensus
    /// layer p2p specification, meaning the input should be treated as untrusted or potentially
    /// adversarial.
    ///
    /// Implementers should take care when acting on the input to this method, specifically
    /// ensuring that the range is limited properly, and that the range boundaries are computed
    /// correctly and without panics.
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> ScrollEngineApiResult<ExecutionPayloadBodiesV1>;

    /// This function will return the `ClientVersionV1` object.
    /// See also:
    /// <https://github.com/ethereum/execution-apis/blob/03911ffc053b8b806123f1fc237184b0092a485a/src/engine/identification.md#engine_getclientversionv1>make fmt
    ///
    ///
    /// - When connected to a single execution client, the consensus client **MUST** receive an
    ///   array with a single `ClientVersionV1` object.
    /// - When connected to multiple execution clients via a multiplexer, the multiplexer **MUST**
    ///   concatenate the responses from each execution client into a single,
    /// flat array before returning the response to the consensus client.
    async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> ScrollEngineApiResult<Vec<ClientVersionV1>>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    async fn exchange_capabilities(
        &self,
        capabilities: Vec<String>,
    ) -> ScrollEngineApiResult<Vec<String>>;
}
