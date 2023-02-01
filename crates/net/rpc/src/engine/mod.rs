use crate::result::rpc_err;
use async_trait::async_trait;
use jsonrpsee::core::{Error, RpcResult as Result};
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::H64;
use reth_rpc_api::EngineApiServer;
use reth_rpc_engine_api::{EngineApiError, EngineApiMessage, EngineApiResult};
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus, TransitionConfiguration,
};
use tokio::sync::{
    mpsc::UnboundedSender,
    oneshot::{self, Receiver},
};

/// The server implementation of Engine API
pub struct EngineApi {
    /// Handle to the consensus engine
    engine_tx: UnboundedSender<EngineApiMessage>,
}

impl std::fmt::Debug for EngineApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApi").finish_non_exhaustive()
    }
}

impl EngineApi {
    async fn delegate_request<T>(
        &self,
        msg: EngineApiMessage,
        rx: Receiver<EngineApiResult<T>>,
    ) -> Result<T> {
        let _ = self.engine_tx.send(msg);
        rx.await.map_err(|err| Error::Custom(err.to_string()))?.map_err(|err| {
            let code = match err {
                EngineApiError::PayloadUnknown => -38001,
                // Any other server error
                _ => jsonrpsee::types::error::INTERNAL_ERROR_CODE,
            };
            rpc_err(code, err.to_string(), None)
        })
    }
}

#[async_trait]
impl EngineApiServer for EngineApi {
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, payload: ExecutionPayload) -> Result<PayloadStatus> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::NewPayload(payload, tx), rx).await
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    async fn new_payload_v2(&self, _payload: ExecutionPayload) -> Result<PayloadStatus> {
        todo!()
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_forkchoiceUpdatedV1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(
            EngineApiMessage::ForkchoiceUpdated(fork_choice_state, payload_attributes, tx),
            rx,
        )
        .await
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_forkchoiceupdatedv2>
    async fn fork_choice_updated_v2(
        &self,
        _fork_choice_state: ForkchoiceState,
        _payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated> {
        todo!()
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    async fn get_payload_v1(&self, payload_id: H64) -> Result<ExecutionPayload> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::GetPayload(payload_id, tx), rx).await
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_getpayloadv2>
    async fn get_payload_v2(&self, _payload_id: H64) -> Result<ExecutionPayload> {
        todo!()
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_exchangeTransitionConfigurationV1>
    async fn exchange_transition_configuration(
        &self,
        config: TransitionConfiguration,
    ) -> Result<TransitionConfiguration> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::ExchangeTransitionConfiguration(config, tx), rx)
            .await
    }
}
