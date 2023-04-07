use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::{BlockHash, U64};
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadBodies, ForkchoiceState, ForkchoiceUpdated,
    PayloadAttributes, PayloadId, PayloadStatus, TransitionConfiguration,
};

#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait EngineApi {
    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    #[method(name = "engine_newPayloadV1")]
    async fn new_payload_v1(&self, payload: ExecutionPayload) -> Result<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_newpayloadv2>
    #[method(name = "engine_newPayloadV2")]
    async fn new_payload_v2(&self, payload: ExecutionPayload) -> Result<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_forkchoiceupdatedv1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    #[method(name = "engine_forkchoiceUpdatedV1")]
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    #[method(name = "engine_forkchoiceUpdatedV2")]
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_getpayloadv1>
    ///
    /// Caution: This should not return the `withdrawals` field
    #[method(name = "engine_getPayloadV1")]
    async fn get_payload_v1(&self, payload_id: PayloadId) -> Result<ExecutionPayload>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_getpayloadv2>
    #[method(name = "engine_getPayloadV2")]
    async fn get_payload_v2(&self, payload_id: PayloadId) -> Result<ExecutionPayload>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    #[method(name = "engine_getPayloadBodiesByHashV1")]
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> Result<ExecutionPayloadBodies>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyrangev1>
    #[method(name = "engine_getPayloadBodiesByRangeV1")]
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> Result<ExecutionPayloadBodies>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_exchangetransitionconfigurationv1>
    #[method(name = "engine_exchangeTransitionConfigurationV1")]
    async fn exchange_transition_configuration(
        &self,
        transition_configuration: TransitionConfiguration,
    ) -> Result<TransitionConfiguration>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    #[method(name = "engine_exchangeCapabilities")]
    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> Result<Vec<String>>;
}
