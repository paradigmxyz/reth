use super::auth::Auth;
use super::json_structures::{JsonRequestBody, JsonResponseBody};
use super::*;
use reqwest::header::CONTENT_TYPE;
pub use reqwest::Client;
use reth_interfaces::consensus::ForkchoiceState;
use reth_rpc_types::engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadInputV2, ExecutionPayloadV1, ForkchoiceUpdated,
    PayloadAttributes, PayloadId,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use url::Url;

const STATIC_ID: u32 = 1;
pub const JSONRPC_VERSION: &str = "2.0";

pub const RETURN_FULL_TRANSACTION_OBJECTS: bool = false;

pub const ETH_GET_BLOCK_BY_NUMBER: &str = "eth_getBlockByNumber";
pub const ETH_GET_BLOCK_BY_NUMBER_TIMEOUT: Duration = Duration::from_secs(1);

pub const ETH_GET_BLOCK_BY_HASH: &str = "eth_getBlockByHash";
pub const ETH_GET_BLOCK_BY_HASH_TIMEOUT: Duration = Duration::from_secs(1);

pub const ETH_SYNCING: &str = "eth_syncing";
pub const ETH_SYNCING_TIMEOUT: Duration = Duration::from_secs(1);

pub const ENGINE_NEW_PAYLOAD_V1: &str = "engine_newPayloadV1";
pub const ENGINE_NEW_PAYLOAD_V2: &str = "engine_newPayloadV2";
pub const ENGINE_NEW_PAYLOAD_TIMEOUT: Duration = Duration::from_secs(8);

pub const ENGINE_GET_PAYLOAD_V1: &str = "engine_getPayloadV1";
pub const ENGINE_GET_PAYLOAD_V2: &str = "engine_getPayloadV2";
pub const ENGINE_GET_PAYLOAD_TIMEOUT: Duration = Duration::from_secs(2);

pub const ENGINE_FORKCHOICE_UPDATED_V1: &str = "engine_forkchoiceUpdatedV1";
pub const ENGINE_FORKCHOICE_UPDATED_V2: &str = "engine_forkchoiceUpdatedV2";
pub const ENGINE_FORKCHOICE_UPDATED_TIMEOUT: Duration = Duration::from_secs(8);

pub const ENGINE_GET_PAYLOAD_BODIES_BY_HASH_V1: &str = "engine_getPayloadBodiesByHashV1";
pub const ENGINE_GET_PAYLOAD_BODIES_BY_RANGE_V1: &str = "engine_getPayloadBodiesByRangeV1";
pub const ENGINE_GET_PAYLOAD_BODIES_TIMEOUT: Duration = Duration::from_secs(10);

pub const ENGINE_EXCHANGE_CAPABILITIES: &str = "engine_exchangeCapabilities";
pub const ENGINE_EXCHANGE_CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(1);

/// This error is returned during a `chainId` call by Geth.
pub const EIP155_ERROR_STR: &str = "chain not synced beyond EIP-155 replay-protection fork block";

pub const METHOD_NOT_FOUND_CODE: i64 = -32601;

pub static CL_CAPABILITIES: &[&str] = &[
    ENGINE_NEW_PAYLOAD_V1,
    ENGINE_NEW_PAYLOAD_V2,
    ENGINE_GET_PAYLOAD_V1,
    ENGINE_GET_PAYLOAD_V2,
    ENGINE_FORKCHOICE_UPDATED_V1,
    ENGINE_FORKCHOICE_UPDATED_V2,
    ENGINE_GET_PAYLOAD_BODIES_BY_HASH_V1,
    ENGINE_GET_PAYLOAD_BODIES_BY_RANGE_V1,
];

pub struct HttpJsonRpc {
    pub client: Client,
    pub url: Url,
    pub execution_timeout_multiplier: u32,
    auth: Option<Auth>,
}

impl HttpJsonRpc {
    pub fn new(url: Url, execution_timeout_multiplier: Option<u32>) -> Result<Self, ClRpcError> {
        Ok(Self {
            client: Client::builder().build()?,
            url,
            execution_timeout_multiplier: execution_timeout_multiplier.unwrap_or(1),
            auth: None,
        })
    }

    pub fn new_with_auth(
        url: Url,
        auth: Auth,
        execution_timeout_multiplier: Option<u32>,
    ) -> Result<Self, ClRpcError> {
        Ok(Self {
            client: Client::builder().build()?,
            url,
            execution_timeout_multiplier: execution_timeout_multiplier.unwrap_or(1),
            auth: Some(auth),
        })
    }

    pub async fn rpc_request<D: DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<D, ClRpcError> {
        let body =
            JsonRequestBody { jsonrpc: JSONRPC_VERSION, method, params, id: json!(STATIC_ID) };

        let mut request = self
            .client
            .post(self.url.clone())
            .timeout(timeout)
            .header(CONTENT_TYPE, "application/json")
            .json(&body);

        // Generate and add a jwt token to the header if auth is defined.
        if let Some(auth) = &self.auth {
            request = request.bearer_auth(auth.generate_token()?);
        };

        let body: JsonResponseBody = request.send().await?.error_for_status()?.json().await?;

        println!("===={:?}", body);

        match (body.result, body.error) {
            (result, None) => serde_json::from_value(result).map_err(Into::into),
            (_, Some(error)) => {
                if error.message.contains(EIP155_ERROR_STR) {
                    Err(ClRpcError::Eip155Failure)
                } else {
                    Err(ClRpcError::ServerMessage { code: error.code, message: error.message })
                }
            }
        }
    }
}

impl std::fmt::Display for HttpJsonRpc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, auth={}", self.url, self.auth.is_some())
    }
}

impl HttpJsonRpc {
    pub async fn upcheck(&self) -> Result<(), ClRpcError> {
        let result: serde_json::Value = self
            .rpc_request(
                ETH_SYNCING,
                json!([]),
                ETH_SYNCING_TIMEOUT * self.execution_timeout_multiplier,
            )
            .await?;

        /*
         * TODO
         *
         * Check the network and chain ids. We omit this to save time for the merge f2f and since it
         * also seems like it might get annoying during development.
         */
        match result.as_bool() {
            Some(false) => Ok(()),
            _ => Err(ClRpcError::IsSyncing),
        }
    }

    pub async fn get_block_by_number<'a>(
        &self,
        query: String,
    ) -> Result<Option<ExecutionBlock>, ClRpcError> {
        let params = json!([query, RETURN_FULL_TRANSACTION_OBJECTS]);

        self.rpc_request(
            ETH_GET_BLOCK_BY_NUMBER,
            params,
            ETH_GET_BLOCK_BY_NUMBER_TIMEOUT * self.execution_timeout_multiplier,
        )
        .await
    }

    pub async fn get_block_by_hash(
        &self,
        block_hash: B256,
    ) -> Result<Option<ExecutionBlock>, ClRpcError> {
        let params = json!([block_hash.to_string(), RETURN_FULL_TRANSACTION_OBJECTS]);

        self.rpc_request(
            ETH_GET_BLOCK_BY_HASH,
            params,
            ETH_GET_BLOCK_BY_HASH_TIMEOUT * self.execution_timeout_multiplier,
        )
        .await
    }

    pub async fn exchange_capabilities(&self) -> Result<(), ClRpcError> {
        let params = json!([CL_CAPABILITIES]);

        let response: Result<HashSet<String>, _> = self
            .rpc_request(
                ENGINE_EXCHANGE_CAPABILITIES,
                params,
                ENGINE_EXCHANGE_CAPABILITIES_TIMEOUT * self.execution_timeout_multiplier,
            )
            .await;

        match response {
            // TODO (mark): rip this out once we are post capella on mainnet
            Err(error) => match error {
                ClRpcError::ServerMessage { code, message: _ } if code == METHOD_NOT_FOUND_CODE => {
                    Ok(())
                }
                _ => Err(error),
            },
            Ok(capabilities) => {
                println!("Capabilities: {:?}", capabilities);
                Ok(())
            }
        }
    }

    pub async fn forkchoice_updated_v1(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        self.forkchoice_updated_version(
            forkchoice_state,
            payload_attributes,
            ENGINE_FORKCHOICE_UPDATED_V1,
        )
        .await
    }

    pub async fn forkchoice_updated_v2(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        self.forkchoice_updated_version(
            forkchoice_state,
            payload_attributes,
            ENGINE_FORKCHOICE_UPDATED_V2,
        )
        .await
    }

    pub async fn forkchoice_updated_version(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
        method_version: &str,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        let json_forkchoice_state = match serde_json::to_string(&forkchoice_state) {
            Ok(json) => json,
            Err(e) => return Err(ClRpcError::Json(e)),
        };
        let json_forkchoice_state: serde_json::Value =
            match serde_json::from_str(&json_forkchoice_state) {
                Ok(json) => json,
                Err(e) => return Err(ClRpcError::Json(e)),
            };

        let params = if let Some(attr) = payload_attributes {
            let val = match serde_json::to_string(&attr) {
                Ok(json) => json,
                Err(e) => return Err(ClRpcError::Json(e)),
            };
            let json: serde_json::Value = match serde_json::from_str(&val) {
                Ok(json) => json,
                Err(e) => return Err(ClRpcError::Json(e)),
            };
            json!([json_forkchoice_state, json])
        } else {
            json!([json_forkchoice_state])
        };

        let response: ForkchoiceUpdated = self
            .rpc_request(
                method_version,
                params,
                ENGINE_FORKCHOICE_UPDATED_TIMEOUT * self.execution_timeout_multiplier,
            )
            .await?;

        Ok(response)
    }

    pub async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> Result<ExecutionPayloadV1, ClRpcError> {
        let params = json!([payload_id.to_string()]);
        let response: ExecutionPayloadV1 = self
            .rpc_request(
                ENGINE_GET_PAYLOAD_V1,
                params,
                ENGINE_GET_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
            )
            .await?;

        Ok(response)
    }

    pub async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> Result<ExecutionPayloadWrapperV2, ClRpcError> {
        let params = json!([payload_id.to_string()]);
        let response: ExecutionPayloadWrapperV2 = self
            .rpc_request(
                ENGINE_GET_PAYLOAD_V2,
                params,
                ENGINE_GET_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
            )
            .await?;

        Ok(response)
    }

    pub async fn new_payload_v1(
        &self,
        payload: ExecutionPayloadV1,
    ) -> Result<PayloadStatus, ClRpcError> {
        let json_payload = match serde_json::to_string(&payload) {
            Ok(json) => json,
            Err(e) => return Err(ClRpcError::Json(e)),
        };
        let json_payload: serde_json::Value = match serde_json::from_str(&json_payload) {
            Ok(json) => json,
            Err(e) => return Err(ClRpcError::Json(e)),
        };

        let params = json!([json_payload]);

        let response: PayloadStatus = self
            .rpc_request(
                ENGINE_NEW_PAYLOAD_V1,
                params,
                ENGINE_NEW_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
            )
            .await?;

        Ok(response)
    }

    pub async fn new_payload_v2(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> Result<PayloadStatus, ClRpcError> {
        let json_payload = match serde_json::to_string(&payload) {
            Ok(json) => json,
            Err(e) => return Err(ClRpcError::Json(e)),
        };
        let json_payload: serde_json::Value = match serde_json::from_str(&json_payload) {
            Ok(json) => json,
            Err(e) => return Err(ClRpcError::Json(e)),
        };

        let params = json!([json_payload]);

        let response: PayloadStatus = self
            .rpc_request(
                ENGINE_NEW_PAYLOAD_V2,
                params,
                ENGINE_NEW_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
            )
            .await?;

        Ok(response)
    }
}

#[cfg(test)]
mod test {
    use alloy_primitives::{Address, B256, U256};
    use reth_interfaces::consensus::ForkchoiceState;
    use reth_rpc_types::{
        engine::{
            ExecutionPayload, ExecutionPayloadEnvelopeV2, ExecutionPayloadFieldV2,
            ExecutionPayloadInputV2, ForkchoiceUpdated, PayloadAttributes, PayloadId,
        },
        ExecutionPayloadV2,
    };
    use serde_json::json;
    use std::{convert::TryFrom, path::PathBuf, str::FromStr};

    use crate::{create_auth_api, engine_api::ExecutionPayloadWrapperV2};
    #[tokio::test]
    async fn forkchoice_updated() {
        // let secret_file = PathBuf::from_str("/work/data/dev1/jwt.hex").unwrap();
        // let api = create_auth_api(secret_file);

        // let forkchoice_state = ForkchoiceState {
        //     head_block_hash: B256::from_str(
        //         "0x0dc46bf51bd99bc67e98765736a395620f7ba3e92aaf8f5b380372e39163105d",
        //     )
        //     .unwrap(),
        //     safe_block_hash: B256::from_str(
        //         "0x0dc46bf51bd99bc67e98765736a395620f7ba3e92aaf8f5b380372e39163105d",
        //     )
        //     .unwrap(),
        //     finalized_block_hash: B256::from_str(
        //         "0x0dc46bf51bd99bc67e98765736a395620f7ba3e92aaf8f5b380372e39163105d",
        //     )
        //     .unwrap(),
        // };

        // let r = api.forkchoice_updated_v2(forkchoice_state, None).await;
        // match r {
        //     Ok(response) => println!("response {:?}", response),
        //     Err(e) => eprintln!("error {:?}", e),
        // }
    }

    // #[tokio::test]
    // async fn exchange_capabilities() {
    //     let secret_file = PathBuf::from_str("/work/data/dev1/jwt.hex").unwrap();
    //     let api = create_auth_api(secret_file);

    //     let r = api.exchange_capabilities().await;
    //     match r {
    //         Ok(response) => println!("response {:?}", response),
    //         Err(e) => eprintln!("error {:?}", e),
    //     }
    // }

    #[tokio::test]
    async fn payloadid() {
        let id = PayloadId::new([42; 8]);
        println!("payloadid {}", id.to_string());
    }

    #[tokio::test]
    async fn payload_v2() {
        let s = r#"{
            "blockValue": "0x0",
            "executionPayload": {
                "baseFeePerGas": "0x342770c0",
                "blockHash": "0xd89efea59f95007ff3af95685b1ab68d3a2d3c0a5913f73c9b22f6c88a0f2d8e",
                "blockNumber": "0x1",
                "extraData": "0x9a726574682f76302e312e302d616c7068612e31322f6c696e7578",
                "feeRecipient": "0x0000000000000000000000000000000000000000",
                "gasLimit": "0x1c9c380",
                "gasUsed": "0x0",
                "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                "parentHash": "0x0dc46bf51bd99bc67e98765736a395620f7ba3e92aaf8f5b380372e39163105d",
                "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
                "stateRoot": "0xc4c96f3e3bdbe1f31718538f9f07fd57b11365bbd89a9d08493ab15f36f4e811",
                "timestamp": "0x658bcae9",
                "transactions": [],
                "withdrawals": [
                    {
                        "address": "0x00000000000000000000000000000000000010f0",
                        "amount": "0x1",
                        "index": "0x0",
                        "validatorIndex": "0x0"
                    }
                ]
            }
        }"#;

        let s2 = r#"{
                "baseFeePerGas": "0x342770c0",
                "blockHash": "0xd89efea59f95007ff3af95685b1ab68d3a2d3c0a5913f73c9b22f6c88a0f2d8e",
                "blockNumber": "0x1",
                "extraData": "0x9a726574682f76302e312e302d616c7068612e31322f6c696e7578",
                "feeRecipient": "0x0000000000000000000000000000000000000000",
                "gasLimit": "0x1c9c380",
                "gasUsed": "0x0",
                "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                "parentHash": "0x0dc46bf51bd99bc67e98765736a395620f7ba3e92aaf8f5b380372e39163105d",
                "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
                "stateRoot": "0xc4c96f3e3bdbe1f31718538f9f07fd57b11365bbd89a9d08493ab15f36f4e811",
                "timestamp": "0x658bcae9",
                "transactions": []
                
            }"#;
        let payload: ExecutionPayloadWrapperV2 = serde_json::from_str(s).unwrap();
        println!("{:?}", payload);

        // let payload: ExecutionPayloadInputV2 = serde_json::from_str(s2).unwrap();
        // println!("{:?}", payload);

        // let anyn_payload: ExecutionPayload = serde_json::from_str(s).unwrap();
        // println!("{:?}", anyn_payload);
    }
}
