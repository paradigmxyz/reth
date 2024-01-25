use crate::{
    error::{PrettyReqwestError, RpcError},
    ClStorage,
};
use alloy_primitives::{B256, U256};
use chrono::format;
use crossbeam_channel::bounded;
use reqwest::StatusCode;
use reth_provider::BlockReaderIdExt;
use reth_rpc_types::{
    engine::{
        ExecutionPayloadInputV2, ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadId,
        PayloadStatus,
    },
    ExecutionPayloadV2,
};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

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

#[derive(Debug)]
pub enum AsyncResultType {
    BlockId(B256),
    PayloadId(PayloadId),
}

#[derive(Debug)]
pub enum ApiServiceError {
    Ok(AsyncResultType),
    MismatchAsyncResultType,
    Timeout(String),
    ApiError(String),
    InvalidState(String),
    UnknownBlock(String),
    UnknownPeer(String),
    NoChainHead,
    BlockNotReady,
}

impl std::fmt::Display for ApiServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Clone)]
pub struct PayloadPair {
    pub payload_id: PayloadId,
    pub block_id: Option<B256>,
}

#[derive(Clone, Default)]
pub struct ApiService {
    api: Arc<HttpJsonRpc>,
    executor: TokioTaskExecutor,
    latest_committed_id: Option<B256>,
    /// key previous_id, value:PayloadPair(payload_id,block_id)
    pairs: HashMap<B256, PayloadPair>,
}

impl ApiService {
    pub fn new(api: Arc<HttpJsonRpc>) -> Self {
        Self {
            api,
            executor: TokioTaskExecutor::default(),
            latest_committed_id: None,
            pairs: HashMap::new(),
        }
    }

    /// Initialize a new block built on the block with the given previous id and
    /// begin adding batches to it. If no previous id is specified, the current
    /// head will be used.
    pub fn initialize_block(&mut self, previous_id: Option<B256>) -> Result<(), ApiServiceError> {
        let api = self.api.clone();
        let (s, r) = crossbeam_channel::bounded(0);

        self.executor.spawn_blocking(Box::pin(async move {
            let block_id = if let Some(block_id) = previous_id {
                block_id
            } else {
                let last_block_hash = match api.get_block_by_number("latest".to_string()).await {
                    Ok(x) => {
                        if let Some(execution_block) = x {
                            execution_block.block_hash
                        } else {
                            // return Err(ApiServiceError::UnknownBlock(
                            //     "get block return none".to_string(),
                            // ));
                            tracing::error!(target:"consensus::cl","ApiService::initialize_block::get_block_by_number return None");
                            let _ = s.try_send(ApiServiceError::UnknownBlock(
                                "get block return none".to_string(),
                            ));
                            return;
                        }
                    }
                    Err(e) => {
                        // return Err(ApiServiceError::ApiError(format!(
                        //     "get block by number error: {:?}",
                        //     e
                        // )));
                        tracing::error!(target:"consensus::cl","ApiService::initialize_block::get_block_by_number return error: {:?}", e);
                        let _ = s.try_send(ApiServiceError::ApiError(format!(
                            "get block by number error: {:?}",
                            e
                        )));
                        return;
                    }
                };
                last_block_hash
            };

            let forkchoice_updated_result = match forkchoice_updated(&api, block_id.clone()).await {
                Ok(x) => x,
                Err(e) => {
                    // return Err(ApiServiceError::ApiError(format!("forkchoice_updated: {:?}", e)));
                    tracing::error!(target:"consensus::cl","ApiService::initialize_block::forkchoice_updated return(error: {:?})", e);
                    let _ = s.try_send(ApiServiceError::ApiError(format!(
                        "forkchoice_updated: {:?}",
                        e
                    )));
                    return;
                }
            };
            if !forkchoice_updated_result.payload_status.status.is_valid() {
                // return Err(ApiServiceError::BlockNotReady);
                tracing::error!(target:"consensus::cl","ApiService::initialize_block::forkchoice_updated return(not valid)");
                let _ = s.try_send(ApiServiceError::BlockNotReady);
                return;
            }
            let _ = s.try_send(ApiServiceError::Ok(AsyncResultType::BlockId(block_id)));
        }));

        let r = r.recv_timeout(std::time::Duration::from_secs(3));
        match r {
            Ok(x) => {
                if let ApiServiceError::Ok(id) = x {
                    match id {
                        AsyncResultType::BlockId(id) => {
                            self.latest_committed_id = Some(id);
                            return Ok(());
                        }
                        _ => {
                            return Err(ApiServiceError::MismatchAsyncResultType);
                        }
                    }
                } else {
                    return Err(x);
                }
            }
            Err(_) => {
                return Err(ApiServiceError::Timeout("initialize_block".to_string()));
            }
        }
    }

    /// Stop adding batches to the current block and return a summary of its
    /// contents.
    pub fn summarize_block(&mut self) -> Result<(), ApiServiceError> {
        let previous_id = match self.latest_committed_id {
            Some(id) => id,
            None => {
                tracing::error!(target:"consensus::cl","ApiService::summarize_block previous_id is None");
                return Err(ApiServiceError::NoChainHead);
            }
        };

        let api = self.api.clone();
        let (s, r) = crossbeam_channel::bounded(0);
        self.executor.spawn_blocking(Box::pin(async move {
            let forkchoice_updated_result =
                match forkchoice_updated_with_attributes(&api, previous_id).await {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::error!(target:"consensus::cl","ApiService::summarize_block::forkchoice_updated_with_attributes return(error: {:?})", e);
                        let _ = s.try_send(ApiServiceError::ApiError(format!(
                            "forkchoice_updated_with_attributes: {:?}",
                            e
                        )));
                        return;
                    }
                };

            if !forkchoice_updated_result.payload_status.status.is_valid() {
                tracing::error!(target:"consensus::cl","ApiService::summarize_block::forkchoice_updated_with_attributes return(not valid)");
                let _ = s.try_send(ApiServiceError::BlockNotReady);
                return;
            }else{
                if let Some(payload_id) = &forkchoice_updated_result.payload_id{
                    // let data = payload_id.serialize();
                }else{

                }
            }
        }));

        let r = r.recv_timeout(std::time::Duration::from_secs(3));
        match r {
            Ok(x) => {
                if let ApiServiceError::Ok(_) = x {
                    return Ok(());
                } else {
                    return Err(x);
                }
            }
            Err(_) => {
                return Err(ApiServiceError::Timeout("summarize_block".to_string()));
            }
        }
    }

    /// Insert the given consensus data into the block and sign it. If this call is successful, the
    /// consensus engine will receive the block afterwards.
    pub fn finalize_block(
        &mut self,
        data: reth_primitives::Bytes,
    ) -> Result<B256, ApiServiceError> {
        let api = self.api.clone();
        let (s, r) = crossbeam_channel::bounded(0);
        self.executor.spawn_blocking(Box::pin(async move {}));

        let r = r.recv_timeout(std::time::Duration::from_secs(3));
        match r {
            Ok(x) => {
                if let ApiServiceError::Ok(_) = x {
                    return Ok(B256::ZERO);
                } else {
                    return Err(x);
                }
            }
            Err(_) => {
                return Err(ApiServiceError::Timeout("initialize_block".to_string()));
            }
        }
    }

    /// Stop adding batches to the current block and abandon it.
    pub fn cancel_block(&mut self) -> Result<(), ApiServiceError> {
        Ok(())
    }

    /// Update the prioritization of blocks to check
    pub fn check_blocks(&mut self, priority: Vec<B256>) -> Result<(), ApiServiceError> {
        Ok(())
    }

    /// Update the block that should be committed
    pub fn commit_block(&mut self, block_id: B256) -> Result<(), ApiServiceError> {
        let api = self.api.clone();
        let (s, r) = crossbeam_channel::bounded(0);
        self.executor.spawn_blocking(Box::pin(async move {}));

        let r = r.recv_timeout(std::time::Duration::from_secs(3));
        match r {
            Ok(x) => {
                if let ApiServiceError::Ok(_) = x {
                    return Ok(());
                } else {
                    return Err(x);
                }
            }
            Err(_) => {
                return Err(ApiServiceError::Timeout("initialize_block".to_string()));
            }
        }
    }

    /// Mark this block as invalid from the perspective of consensus
    pub fn fail_block(&mut self, block_id: B256) -> Result<(), ApiServiceError> {
        Ok(())
    }

    pub fn announce_block(&mut self, block_id: B256) -> Result<(), ApiServiceError> {
        //broadcast new block hash after commit
        Ok(())
    }

    pub fn sync_block(&mut self, block_id: B256) -> Result<(), ApiServiceError> {
        //broadcast new block hash after commit
        Ok(())
    }
}
