use super::auth::Auth;
use super::json_structures::{JsonRequestBody, JsonResponseBody};
use super::*;
pub use reqwest::blocking::Client as ClientBlocking;
use reqwest::header::CONTENT_TYPE;
use reth_interfaces::consensus::ForkchoiceState;
use reth_rpc_types::engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadInputV2, ExecutionPayloadV1, ForkchoiceUpdated,
    PayloadAttributes, PayloadId,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use url::Url;

pub struct HttpJsonRpcSync {
    pub client: ClientBlocking,
    pub url: Url,
    pub execution_timeout_multiplier: u32,
    auth: Option<Auth>,
}

impl Default for HttpJsonRpcSync {
    fn default() -> Self {
        Self::new(Url::parse("http://127.0.0.1:8551/").unwrap(), None).unwrap()
    }
}

impl HttpJsonRpcSync {
    pub fn new(url: Url, execution_timeout_multiplier: Option<u32>) -> Result<Self, ClRpcError> {
        Ok(Self {
            client: ClientBlocking::new(),
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
            client: ClientBlocking::new(),
            url,
            execution_timeout_multiplier: execution_timeout_multiplier.unwrap_or(1),
            auth: Some(auth),
        })
    }

    pub fn rpc_request<D: DeserializeOwned>(
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

        let body: JsonResponseBody = request.send()?.error_for_status()?.json()?;

        // println!("===={:?}", body);

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

impl std::fmt::Display for HttpJsonRpcSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, auth={}", self.url, self.auth.is_some())
    }
}

impl HttpJsonRpcSync {
    pub fn upcheck(&self) -> Result<(), ClRpcError> {
        let result: serde_json::Value = self.rpc_request(
            ETH_SYNCING,
            json!([]),
            ETH_SYNCING_TIMEOUT * self.execution_timeout_multiplier,
        )?;

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

    pub fn block_number(&self) -> Result<U256, ClRpcError> {
        let params = json!([]);
        self.rpc_request(
            ETH_BLOCK_NUMBER,
            params,
            ETH_BLOCK_NUMBER_TIMEOUT * self.execution_timeout_multiplier,
        )
    }

    pub fn get_block_by_number<'a>(
        &self,
        query: String,
    ) -> Result<Option<ExecutionBlock>, ClRpcError> {
        let params = json!([query, RETURN_FULL_TRANSACTION_OBJECTS]);

        self.rpc_request(
            ETH_GET_BLOCK_BY_NUMBER,
            params,
            ETH_GET_BLOCK_BY_NUMBER_TIMEOUT * self.execution_timeout_multiplier,
        )
    }

    pub fn get_block_by_hash(
        &self,
        block_hash: B256,
    ) -> Result<Option<ExecutionBlock>, ClRpcError> {
        let params = json!([block_hash.to_string(), RETURN_FULL_TRANSACTION_OBJECTS]);

        self.rpc_request(
            ETH_GET_BLOCK_BY_HASH,
            params,
            ETH_GET_BLOCK_BY_HASH_TIMEOUT * self.execution_timeout_multiplier,
        )
    }

    pub fn exchange_capabilities(&self) -> Result<(), ClRpcError> {
        let params = json!([CL_CAPABILITIES]);

        let response: Result<HashSet<String>, _> = self.rpc_request(
            ENGINE_EXCHANGE_CAPABILITIES,
            params,
            ENGINE_EXCHANGE_CAPABILITIES_TIMEOUT * self.execution_timeout_multiplier,
        );

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

    pub fn forkchoice_updated_v1(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        self.forkchoice_updated_version(
            forkchoice_state,
            payload_attributes,
            ENGINE_FORKCHOICE_UPDATED_V1,
        )
    }

    pub fn forkchoice_updated_v2(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        self.forkchoice_updated_version(
            forkchoice_state,
            payload_attributes,
            ENGINE_FORKCHOICE_UPDATED_V2,
        )
    }

    pub fn forkchoice_updated_version(
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

        let response: ForkchoiceUpdated = self.rpc_request(
            method_version,
            params,
            ENGINE_FORKCHOICE_UPDATED_TIMEOUT * self.execution_timeout_multiplier,
        )?;

        Ok(response)
    }

    pub fn get_payload_v1(&self, payload_id: PayloadId) -> Result<ExecutionPayloadV1, ClRpcError> {
        let params = json!([payload_id.to_string()]);
        let response: ExecutionPayloadV1 = self.rpc_request(
            ENGINE_GET_PAYLOAD_V1,
            params,
            ENGINE_GET_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
        )?;

        Ok(response)
    }

    pub fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> Result<ExecutionPayloadWrapperV2, ClRpcError> {
        let params = json!([payload_id.to_string()]);
        let response: ExecutionPayloadWrapperV2 = self.rpc_request(
            ENGINE_GET_PAYLOAD_V2,
            params,
            ENGINE_GET_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
        )?;

        Ok(response)
    }

    pub fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> Result<PayloadStatus, ClRpcError> {
        let json_payload = match serde_json::to_string(&payload) {
            Ok(json) => json,
            Err(e) => return Err(ClRpcError::Json(e)),
        };
        let json_payload: serde_json::Value = match serde_json::from_str(&json_payload) {
            Ok(json) => json,
            Err(e) => return Err(ClRpcError::Json(e)),
        };

        let params = json!([json_payload]);

        let response: PayloadStatus = self.rpc_request(
            ENGINE_NEW_PAYLOAD_V1,
            params,
            ENGINE_NEW_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
        )?;

        Ok(response)
    }

    pub fn new_payload_v2(
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

        let response: PayloadStatus = self.rpc_request(
            ENGINE_NEW_PAYLOAD_V2,
            params,
            ENGINE_NEW_PAYLOAD_TIMEOUT * self.execution_timeout_multiplier,
        )?;

        Ok(response)
    }
}
