use crate::{QueryError, QueryRequest, QueryResponse, QueryService};
use jsonrpsee::{
    core::RpcResult,
    proc_macros::rpc,
    types::{ErrorCode, ErrorObjectOwned},
};

#[rpc(server, namespace = "pureth")]
pub trait PurethApi {
    #[method(name = "query")]
    fn query(&self, request: QueryRequest) -> RpcResult<QueryResponse>;
}

#[derive(Debug)]
pub struct PurethRpc {
    service: QueryService,
}

impl PurethRpc {
    pub const fn new(service: QueryService) -> Self {
        Self { service }
    }
}

impl PurethApiServer for PurethRpc {
    fn query(&self, request: QueryRequest) -> RpcResult<QueryResponse> {
        self.service.query(request).map_err(rpc_error)
    }
}

fn rpc_error(error: QueryError) -> ErrorObjectOwned {
    let code = match error {
        QueryError::UnsupportedObject |
        QueryError::UnsupportedSchema |
        QueryError::ProofRequired |
        QueryError::InvalidPath(_) |
        QueryError::UnsupportedPath(_) |
        QueryError::Provider(_) |
        QueryError::Proof(crate::ProofAccessError::Resolution(_)) => ErrorCode::InvalidParams,
        QueryError::Proof(_) | QueryError::InvalidResponse(_) => ErrorCode::InternalError,
    };

    ErrorObjectOwned::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use jsonrpsee::core::server::Methods;
    use reth_pureth_receipt::SINGLETON_BLOCK_HASH;

    fn module() -> Methods {
        PurethRpc::new(QueryService::new().unwrap()).into_rpc().into()
    }

    #[tokio::test]
    async fn in_process_rpc_uses_pureth_query() {
        let module = module();
        assert!(module.method("pureth_query").is_some());

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "pureth_query",
            "params": {
                "request": {
                    "block_hash": SINGLETON_BLOCK_HASH,
                    "object": "receipts",
                    "schema_id": "pureth-receipt-v0",
                    "path": "[0].logs[0].address",
                    "include_proof": true
                }
            }
        });
        let (response, _) = module.raw_json_request(&request.to_string(), 1).await.unwrap();
        let response: serde_json::Value = serde_json::from_str(response.get()).unwrap();

        let result: QueryResponse = serde_json::from_value(response["result"].clone()).unwrap();
        let expected = QueryService::new()
            .unwrap()
            .query(QueryRequest {
                block_hash: SINGLETON_BLOCK_HASH,
                object: "receipts".to_owned(),
                schema_id: crate::SCHEMA_ID.to_owned(),
                path: "[0].logs[0].address".to_owned(),
                include_proof: true,
            })
            .unwrap();
        assert_eq!(result, expected);
        assert_eq!(response["result"]["gindex"], "576");
        assert_eq!(response["result"]["proof_format"], "merkle_branch_v0");
        crate::verify_receipt_log_address(
            &result.schema_id,
            &result.path,
            result.value_ssz.as_ref(),
            &result.proof,
            result.root,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn rpc_maps_request_failures_to_invalid_params() {
        let module = module();
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "pureth_query",
            "params": {
                "request": {
                    "block_hash": SINGLETON_BLOCK_HASH,
                    "object": "receipts",
                    "schema_id": "pureth-receipt-v0",
                    "path": "[0].logs[0].address",
                    "include_proof": false
                }
            }
        });
        let (response, _) = module.raw_json_request(&request.to_string(), 1).await.unwrap();
        let response: serde_json::Value = serde_json::from_str(response.get()).unwrap();

        assert_eq!(response["error"]["code"], -32602);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "pureth_query",
            "params": {
                "request": {
                    "block_hash": B256::repeat_byte(0xff),
                    "object": "receipts",
                    "schema_id": "pureth-receipt-v0",
                    "path": "[0].logs[0].address",
                    "include_proof": true
                }
            }
        });
        let (response, _) = module.raw_json_request(&request.to_string(), 1).await.unwrap();
        let response: serde_json::Value = serde_json::from_str(response.get()).unwrap();

        assert_eq!(response["error"]["code"], -32602);
    }
}
