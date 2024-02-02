use crate::{
    error::{PrettyReqwestError, RpcError},
    ClStorage,
};
use alloy_primitives::{B256, U256};
use chrono::format;
use reqwest::StatusCode;
use reth_provider::BlockReaderIdExt;
use reth_rpc_types::{
    engine::{
        ExecutionPayloadInputV2, ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadId,
        PayloadStatus,
    },
    ExecutionPayloadV2,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

//use self::http::HttpJsonRpc;
use self::http_blocking::HttpJsonRpc;

pub mod auth;
//pub mod http;
pub mod http_blocking;
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

pub fn forkchoice_updated(
    api: &Arc<HttpJsonRpc>,
    last_block: B256,
) -> Result<ForkchoiceUpdated, ClRpcError> {
    let forkchoice_state = ForkchoiceState {
        head_block_hash: last_block,
        finalized_block_hash: last_block,
        safe_block_hash: last_block,
    };

    let response = api.forkchoice_updated_v2(forkchoice_state, None)?;
    Ok(response)
}

pub fn forkchoice_updated_with_attributes(
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
    let response = api.forkchoice_updated_v2(forkchoice_state, Some(p))?;
    Ok(response)
}

pub fn new_payload(
    api: &Arc<HttpJsonRpc>,
    execution_payload: ExecutionPayloadWrapperV2,
) -> Result<PayloadStatus, ClRpcError> {
    let input = ExecutionPayloadInputV2 {
        execution_payload: execution_payload.execution_payload.payload_inner.clone(),
        withdrawals: Some(execution_payload.execution_payload.withdrawals.clone()),
    };
    let response = api.new_payload_v2(input)?;

    Ok(response)
}

#[derive(Debug)]
pub enum AsyncResultType {
    BlockId(B256),
    ForkchoiceUpdated(ForkchoiceUpdated),
    ExecutionPayload(ExecutionPayloadWrapperV2),
}

#[derive(Debug)]
pub enum ApiServiceError {
    Ok(AsyncResultType),
    MismatchAsyncResultType,
    ApiError(String),
    InvalidState(String),
    UnknownBlock(String),
    UnknownPeer(String),
    NoChainHead,
    BlockNotReady,
    Other(String),
}

impl std::fmt::Display for ApiServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Clone)]
pub struct PayloadPair {
    pub payload_id: PayloadId,
    pub execution_payload: Option<ExecutionPayloadWrapperV2>,
}

pub struct ApiService {
    api: Arc<HttpJsonRpc>,
    rt: tokio::runtime::Runtime,
    latest_committed_id: Option<B256>,
    /// key latest_committed_id, value:payload_id
    next_payload_id_pairs: HashMap<B256, PayloadId>,
    /// key proposing block_id, value:ExecutionPayloadWrapperV2
    proposing_payload_pairs: HashMap<B256, (PayloadId, ExecutionPayloadWrapperV2)>,
}

impl ApiService {
    pub fn new(api: Arc<HttpJsonRpc>) -> Self {
        Self {
            api,
            rt: tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap(),
            latest_committed_id: None,
            next_payload_id_pairs: HashMap::new(),
            proposing_payload_pairs: HashMap::new(),
        }
    }

    /// Initialize a new block built on the block with the given previous id and
    /// begin adding batches to it. If no previous id is specified, the current
    /// head will be used.
    pub fn initialize_block(&mut self, previous_id: Option<B256>) -> Result<(), ApiServiceError> {
        let api = self.api.clone();

        let block_id = if let Some(block_id) = previous_id {
            block_id
        } else {
            let last_block_hash = match api.get_block_by_number("latest".to_string()) {
                Ok(x) => {
                    if let Some(execution_block) = x {
                        execution_block.block_hash
                    } else {
                        tracing::error!(target:"consensus::cl","ApiService::initialize_block::get_block_by_number return None");
                        return Err(ApiServiceError::UnknownBlock(
                            "get block return none".to_string(),
                        ));
                    }
                }
                Err(e) => {
                    tracing::error!(target:"consensus::cl","ApiService::initialize_block::get_block_by_number return error: {:?}", e);
                    return Err(ApiServiceError::ApiError(format!(
                        "get block by number error: {:?}",
                        e
                    )));
                }
            };
            last_block_hash
        };

        let forkchoice_updated_result = match forkchoice_updated(&api, block_id.clone()) {
            Ok(x) => x,
            Err(e) => {
                // return Err(ApiServiceError::ApiError(format!("forkchoice_updated: {:?}", e)));
                tracing::error!(target:"consensus::cl","ApiService::initialize_block::forkchoice_updated return(error: {:?})", e);
                return Err(ApiServiceError::ApiError(format!("forkchoice_updated: {:?}", e)));
            }
        };
        if !forkchoice_updated_result.payload_status.status.is_valid() {
            // return Err(ApiServiceError::BlockNotReady);
            tracing::error!(target:"consensus::cl","ApiService::initialize_block::forkchoice_updated return(not valid)");
            return Err(ApiServiceError::BlockNotReady);
        }
        self.latest_committed_id = Some(block_id);
        return Ok(());
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

        let forkchoice_updated = match forkchoice_updated_with_attributes(&api, previous_id) {
            Ok(x) => x,
            Err(e) => {
                tracing::error!(target:"consensus::cl","ApiService::summarize_block::forkchoice_updated_with_attributes return(error: {:?})", e);
                return Err(ApiServiceError::ApiError(format!(
                    "forkchoice_updated_with_attributes: {:?}",
                    e
                )));
            }
        };

        if !forkchoice_updated.payload_status.status.is_valid() {
            tracing::error!(target:"consensus::cl","ApiService::summarize_block::forkchoice_updated_with_attributes return(not valid)");
            return Err(ApiServiceError::BlockNotReady);
        } else {
            if let Some(payload_id) = &forkchoice_updated.payload_id {
                self.next_payload_id_pairs.insert(previous_id, payload_id.clone());
                return Ok(());
            } else {
                tracing::error!(target:"consensus::cl","ApiService::summarize_block::forkchoice_updated_with_attributes payload_id is None");
                return Err(ApiServiceError::BlockNotReady);
            }
        }
    }

    /// Insert the given consensus data into the block and sign it. If this call is successful, the
    /// consensus engine will receive the block afterwards.
    pub fn finalize_block(&mut self) -> Result<ExecutionPayloadWrapperV2, ApiServiceError> {
        let (previous_id, payload_id) = match self.latest_committed_id {
            Some(id) => {
                if let Some(payload_id) = self.next_payload_id_pairs.get(&id) {
                    (id, payload_id.clone())
                } else {
                    tracing::error!(target:"consensus::cl","ApiService::finalize_block payload_id is None");
                    return Err(ApiServiceError::BlockNotReady);
                }
            }
            None => {
                tracing::error!(target:"consensus::cl","ApiService::finalize_block previous_id is None");
                return Err(ApiServiceError::NoChainHead);
            }
        };

        let api = self.api.clone();

        match api.get_payload_v2(payload_id) {
            Ok(playload) => {
                let block_id = playload.execution_payload.payload_inner.block_hash;
                let last_block_id = playload.execution_payload.payload_inner.parent_hash;

                // check parent_hash consistent
                if last_block_id != previous_id {
                    panic!("TODO: check parent_hash consistent");
                }

                self.proposing_payload_pairs.insert(block_id, (payload_id, playload.clone()));

                return Ok(playload);
            }
            Err(e) => {
                tracing::error!(target:"consensus::cl","ApiService::finalize_block::get_payload_v2 return(error: {:?})", e);
                return Err(ApiServiceError::ApiError(format!("get_payload_v2: {:?}", e)));
            }
        };
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
        let (payload_id, execution_payload) = match self.proposing_payload_pairs.get(&block_id) {
            Some(payload) => payload.clone(),
            None => {
                return Err(ApiServiceError::BlockNotReady);
            }
        };

        let api = self.api.clone();

        let payload_status = match new_payload(&api, execution_payload) {
            Ok(payload_status) => {
                let forkchoice_updated =
                    ForkchoiceUpdated { payload_status, payload_id: Some(payload_id) };
                if forkchoice_updated.payload_status.status.is_valid() {
                    if let Some(latest_valid_hash) =
                        &forkchoice_updated.payload_status.latest_valid_hash
                    {
                        return Ok(());
                    } else {
                        tracing::error!(target:"consensus::cl","ApiService::commit_block::new_payload latest_valid_hash is None");
                        return Err(ApiServiceError::BlockNotReady);
                    }
                } else {
                    tracing::error!(target:"consensus::cl","ApiService::commit_block::new_payload return(not valid)");
                    return Err(ApiServiceError::BlockNotReady);
                }
            }
            Err(e) => {
                tracing::error!(target:"consensus::cl","ApiService::commit_block::new_payload return(error: {:?})", e);
                return Err(ApiServiceError::ApiError(format!("new_payload: {:?}", e)));
            }
        };
    }

    /// Mark this block as invalid from the perspective of consensus
    pub fn fail_block(&mut self, block_id: B256) -> Result<(), ApiServiceError> {
        Ok(())
    }

    pub fn announce_block(&mut self, block_id: B256) -> Result<(), ApiServiceError> {
        //broadcast new block hash after commit
        self.sync_block(block_id)
    }

    pub fn sync_block(&mut self, block_id: B256) -> Result<(), ApiServiceError> {
        let api = self.api.clone();

        let forkchoice_updated_result = match forkchoice_updated(&api, block_id.clone()) {
            Ok(x) => x,
            Err(e) => {
                // return Err(ApiServiceError::ApiError(format!("forkchoice_updated: {:?}", e)));
                tracing::error!(target:"consensus::cl","ApiService::sync_block::forkchoice_updated return(error: {:?})", e);
                return Err(ApiServiceError::ApiError(format!("forkchoice_updated: {:?}", e)));
            }
        };
        if !forkchoice_updated_result.payload_status.status.is_valid() {
            // return Err(ApiServiceError::BlockNotReady);
            tracing::error!(target:"consensus::cl","ApiService::sync_block::forkchoice_updated return(not valid)");
            return Err(ApiServiceError::BlockNotReady);
        }

        self.latest_committed_id = Some(block_id);
        return Ok(());
    }
}
