use std::sync::Arc;

use crate::result::rpc_err;
use async_trait::async_trait;
use jsonrpsee::{
    core::{Error, RpcResult as Result},
    types::error::INVALID_PARAMS_CODE,
};
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{BlockHash, BlockNumber, ChainSpec, Hardfork, H64};
use reth_rpc_api::EngineApiServer;
use reth_rpc_engine_api::{
    EngineApiError, EngineApiHandle, EngineApiMessage, EngineApiMessageVersion, EngineApiResult,
    REQUEST_TOO_LARGE_CODE, UNKNOWN_PAYLOAD_CODE,
};
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadBodies, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
    TransitionConfiguration, CAPABILITIES,
};
use tokio::sync::oneshot::{self, Receiver};

fn to_rpc_error<E: Into<EngineApiError>>(error: E) -> Error {
    let error = error.into();
    let code = match error {
        EngineApiError::InvalidParams => INVALID_PARAMS_CODE,
        EngineApiError::PayloadUnknown => UNKNOWN_PAYLOAD_CODE,
        EngineApiError::PayloadRequestTooLarge { .. } => REQUEST_TOO_LARGE_CODE,
        // Any other server error
        _ => jsonrpsee::types::error::INTERNAL_ERROR_CODE,
    };
    rpc_err(code, error.to_string(), None)
}

/// The server implementation of Engine API
pub struct EngineApi {
    /// Chain spec
    chain_spec: Arc<ChainSpec>,
    /// Handle to the engine API implementation.
    engine_tx: EngineApiHandle,
}

impl EngineApi {
    /// Creates a new instance of [EngineApi].
    pub fn new(chain_spec: Arc<ChainSpec>, engine_tx: EngineApiHandle) -> Self {
        Self { chain_spec, engine_tx }
    }
}

impl std::fmt::Debug for EngineApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApi").finish_non_exhaustive()
    }
}

impl EngineApi {
    /// Validates the presence of the `withdrawals` field according to the payload timestamp.
    /// After Shanghai, withdrawals field must be [Some].
    /// Before Shanghai, withdrawals field must be [None];
    fn validate_withdrawals_presence(
        &self,
        version: EngineApiMessageVersion,
        timestamp: u64,
        has_withdrawals: bool,
    ) -> EngineApiResult<()> {
        let is_shanghai = self.chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(timestamp);

        match version {
            EngineApiMessageVersion::V1 => {
                if is_shanghai || has_withdrawals {
                    return Err(EngineApiError::InvalidParams)
                }
            }
            EngineApiMessageVersion::V2 => {
                let shanghai_with_no_withdrawals = is_shanghai && !has_withdrawals;
                let not_shanghai_with_withdrawals = !is_shanghai && has_withdrawals;
                if shanghai_with_no_withdrawals || not_shanghai_with_withdrawals {
                    return Err(EngineApiError::InvalidParams)
                }
            }
        };

        Ok(())
    }

    async fn delegate_request<T, E: Into<EngineApiError>>(
        &self,
        msg: EngineApiMessage,
        rx: Receiver<std::result::Result<T, E>>,
    ) -> Result<T> {
        let _ = self.engine_tx.send(msg);
        rx.await.map_err(|err| Error::Custom(err.to_string()))?.map_err(|err| to_rpc_error(err))
    }
}

#[async_trait]
impl EngineApiServer for EngineApi {
    /// Handler for `engine_getPayloadV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, payload: ExecutionPayload) -> Result<PayloadStatus> {
        self.validate_withdrawals_presence(
            EngineApiMessageVersion::V1,
            payload.timestamp.as_u64(),
            payload.withdrawals.is_some(),
        )
        .map_err(to_rpc_error)?;
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::NewPayload(payload, tx), rx).await
    }

    /// Handler for `engine_getPayloadV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    async fn new_payload_v2(&self, payload: ExecutionPayload) -> Result<PayloadStatus> {
        self.validate_withdrawals_presence(
            EngineApiMessageVersion::V2,
            payload.timestamp.as_u64(),
            payload.withdrawals.is_some(),
        )
        .map_err(to_rpc_error)?;
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::NewPayload(payload, tx), rx).await
    }

    /// Handler for `engine_forkchoiceUpdatedV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_forkchoiceUpdatedV1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated> {
        if let Some(ref attrs) = payload_attributes {
            self.validate_withdrawals_presence(
                EngineApiMessageVersion::V1,
                attrs.timestamp.as_u64(),
                attrs.withdrawals.is_some(),
            )
            .map_err(to_rpc_error)?;
        }
        let (tx, rx) = oneshot::channel();
        self.delegate_request(
            EngineApiMessage::ForkchoiceUpdated(fork_choice_state, payload_attributes, tx),
            rx,
        )
        .await
    }

    /// Handler for `engine_forkchoiceUpdatedV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_forkchoiceupdatedv2>
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated> {
        if let Some(ref attrs) = payload_attributes {
            self.validate_withdrawals_presence(
                EngineApiMessageVersion::V2,
                attrs.timestamp.as_u64(),
                attrs.withdrawals.is_some(),
            )
            .map_err(to_rpc_error)?;
        }
        let (tx, rx) = oneshot::channel();
        self.delegate_request(
            EngineApiMessage::ForkchoiceUpdated(fork_choice_state, payload_attributes, tx),
            rx,
        )
        .await
    }

    /// Handler for `engine_getPayloadV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    async fn get_payload_v1(&self, payload_id: H64) -> Result<ExecutionPayload> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::GetPayload(payload_id, tx), rx).await
    }

    /// Handler for `engine_getPayloadV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_getpayloadv2>
    async fn get_payload_v2(&self, payload_id: H64) -> Result<ExecutionPayload> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::GetPayload(payload_id, tx), rx).await
    }

    /// Handler for `engine_getPayloadBodiesByHashV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> Result<ExecutionPayloadBodies> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::GetPayloadBodiesByHash(block_hashes, tx), rx).await
    }

    /// Handler for `engine_getPayloadBodiesByRangeV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyrangev1>
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> Result<ExecutionPayloadBodies> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::GetPayloadBodiesByRange(start, count, tx), rx).await
    }

    /// Handler for `engine_exchangeTransitionConfigurationV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_exchangeTransitionConfigurationV1>
    async fn exchange_transition_configuration(
        &self,
        config: TransitionConfiguration,
    ) -> Result<TransitionConfiguration> {
        let (tx, rx) = oneshot::channel();
        self.delegate_request(EngineApiMessage::ExchangeTransitionConfiguration(config, tx), rx)
            .await
    }

    /// Handler for `engine_exchangeCapabilitiesV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    async fn exchange_capabilities(&self, _capabilities: Vec<String>) -> Result<Vec<String>> {
        Ok(CAPABILITIES.into_iter().map(str::to_owned).collect())
    }
}
