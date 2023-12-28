use std::sync::Arc;

use alloy_primitives::B256;
use reth_rpc_types::engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadFieldV2, ExecutionPayloadInputV2, ForkchoiceState,
    ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use tracing::info;

use crate::engine_api::{http::HttpJsonRpc, ClRpcError, ExecutionPayloadWrapperV2};

pub struct MyConsensusEngine;

impl MyConsensusEngine {
    pub async fn forkchoice_updated(
        api: &Arc<HttpJsonRpc>,
        last_block: B256,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: last_block,
            finalized_block_hash: last_block,
            safe_block_hash: last_block,
        };

        let response = api.forkchoice_updated_v2(forkchoice_state, None).await?;
        Ok(response)
    }

    pub async fn forkchoice_updated_with_attributes(
        api: &Arc<HttpJsonRpc>,
        last_block: B256,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: last_block,
            finalized_block_hash: last_block,
            safe_block_hash: last_block,
        };

        let data = r#"
            {
                "timestamp": "0x658967b8",
                "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "suggestedFeeRecipient": "0x0000000000000000000000000000000000000000",
                "withdrawals": [
                    {
                        "index": "0x00",
                        "validatorIndex": "0x00",
                        "address": "0x00000000000000000000000000000000000010f0",
                        "amount": "0x1"
                    }
                ]
            }"#;
        let mut p: PayloadAttributes = serde_json::from_str(data).unwrap();
        let dt = chrono::prelude::Local::now();
        p.timestamp = dt.timestamp() as u64;
        let response = api.forkchoice_updated_v2(forkchoice_state, Some(p)).await?;
        Ok(response)
    }

    pub async fn new_payload(
        api: &Arc<HttpJsonRpc>,
        execution_payload: ExecutionPayloadWrapperV2,
    ) -> Result<PayloadStatus, ClRpcError> {
        let input = ExecutionPayloadInputV2 {
            execution_payload: execution_payload.execution_payload.payload_inner.clone(),
            withdrawals: Some(execution_payload.execution_payload.withdrawals.clone()),
        };
        let response = api.new_payload_v2(input).await?;

        Ok(response)
    }

    // pub fn process_forkchoice_updated(
    //     api: &Arc<HttpJsonRpc>,
    //     last_block: B256,
    //     response: ForkchoiceUpdated,
    // ) {
    //     info!(target: "consensus::cl","forkchoice state response {:?}", response);
    //     if response.payload_status.status.is_valid() {
    //         match response.payload_id {
    //             Some(payload_id) => {}
    //             None => {}
    //         }
    //     }
    // }
}
