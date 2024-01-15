use crate::error::{PrettyReqwestError, RpcError};
use alloy_primitives::{B256, U256};
use reqwest::StatusCode;
use reth_rpc_types::{
    engine::{
        ExecutionPayloadInputV2, ForkchoiceState, ForkchoiceUpdated, PayloadAttributes,
        PayloadStatus,
    },
    ExecutionPayloadV2,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use self::http::HttpJsonRpc;

pub mod auth;
pub mod http;
pub mod json_structures;

pub const LATEST_TAG: &str = "latest";

#[derive(Debug)]
pub enum ClRpcError {
    HttpClient(PrettyReqwestError),
    Auth(auth::Error),
    BadResponse(String),
    RequestFailed(String),
    InvalidExecutePayloadResponse(&'static str),
    JsonRpc(RpcError),
    Json(serde_json::Error),
    ServerMessage { code: i64, message: String },
    Eip155Failure,
    IsSyncing,
    // ExecutionBlockNotFound(ExecutionBlockHash),
    // ExecutionHeadBlockNotFound,
    // ParentHashEqualsBlockHash(ExecutionBlockHash),
    // PayloadIdUnavailable,
    // TransitionConfigurationMismatch,
    // PayloadConversionLogicFlaw,
    // DeserializeTransaction(ssz_types::Error),
    // DeserializeTransactions(ssz_types::Error),
    // DeserializeWithdrawals(ssz_types::Error),
    // BuilderApi(builder_client::Error),
    // IncorrectStateVariant,
    // RequiredMethodUnsupported(&'static str),
    // UnsupportedForkVariant(String),
    // BadConversion(String),
    // RlpDecoderError(rlp::DecoderError),
}

impl From<reqwest::Error> for ClRpcError {
    fn from(e: reqwest::Error) -> Self {
        if matches!(e.status(), Some(StatusCode::UNAUTHORIZED) | Some(StatusCode::FORBIDDEN)) {
            ClRpcError::Auth(auth::Error::InvalidToken)
        } else {
            ClRpcError::HttpClient(e.into())
        }
    }
}

impl From<serde_json::Error> for ClRpcError {
    fn from(e: serde_json::Error) -> Self {
        ClRpcError::Json(e)
    }
}

impl From<auth::Error> for ClRpcError {
    fn from(e: auth::Error) -> Self {
        ClRpcError::Auth(e)
    }
}

// impl From<builder_client::Error> for Error {
//     fn from(e: builder_client::Error) -> Self {
//         Error::BuilderApi(e)
//     }
// }

// impl From<rlp::DecoderError> for Error {
//     fn from(e: rlp::DecoderError) -> Self {
//         Error::RlpDecoderError(e)
//     }
// }

/// Representation of an exection block with enough detail to determine the terminal PoW block.
///
/// See `get_pow_block_hash_at_total_difficulty`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionBlock {
    #[serde(rename = "hash")]
    pub block_hash: B256,
    #[serde(rename = "number", with = "serde_utils::u64_hex_be")]
    pub block_number: u64,
    pub parent_hash: B256,
    pub total_difficulty: U256,
    #[serde(with = "serde_utils::u64_hex_be")]
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadWrapperV2 {
    pub execution_payload: ExecutionPayloadV2,
    /// The expected value to be received by the feeRecipient in wei
    pub block_value: U256,
}

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
