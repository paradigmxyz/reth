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
    #[method(name = "engine_newPayloadV1")]
    async fn new_payload(&self, payload: ExecutionPayload) -> Result<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_forkchoiceUpdatedV1>
    #[method(name = "engine_forkchoiceUpdatedV1")]
    async fn fork_choice_updated(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated>;

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_getPayloadV1>
    #[method(name = "engine_getPayloadV1")]
    async fn get_payload(&self, payload_id: H64) -> Result<ExecutionPayload>;

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_exchangeTransitionConfigurationV1>
    #[method(name = "engine_exchangeTransitionConfigurationV1")]
    async fn exchange_transition_configuration(
        &self,
        transition_configuration: TransitionConfiguration,
    ) -> Result<TransitionConfiguration>;
}
