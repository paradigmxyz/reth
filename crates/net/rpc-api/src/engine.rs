use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::H64;
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
    TransitionConfiguration,
};

#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait EngineApi {
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    #[method(name = "engine_newPayloadV1")]
    async fn new_payload_v1(&self, payload: ExecutionPayload) -> Result<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    #[method(name = "engine_newPayloadV2")]
    async fn new_payload_v2(&self, payload: ExecutionPayload) -> Result<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_forkchoiceUpdatedV1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    #[method(name = "engine_forkchoiceUpdatedV1")]
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated>;

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_forkchoiceupdatedv2>
    #[method(name = "engine_forkchoiceUpdatedV2")]
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated>;

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    #[method(name = "engine_getPayloadV1")]
    async fn get_payload_v1(&self, payload_id: H64) -> Result<ExecutionPayload>;

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_getpayloadv2>
    #[method(name = "engine_getPayloadV2")]
    async fn get_payload_v2(&self, payload_id: H64) -> Result<ExecutionPayload>;

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_exchangeTransitionConfigurationV1>
    #[method(name = "engine_exchangeTransitionConfigurationV1")]
    async fn exchange_transition_configuration(
        &self,
        transition_configuration: TransitionConfiguration,
    ) -> Result<TransitionConfiguration>;
}
