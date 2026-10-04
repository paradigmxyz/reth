use crate::{QueryError, QueryHandler, QueryRequest, QueryResponse};
use alloy_rpc_types_eth::error::EthRpcErrorCode;
use jsonrpsee::{
    core::RpcResult,
    proc_macros::rpc,
    types::{ErrorCode, ErrorObjectOwned},
};
use reth_pureth_receipt::LookupError;

#[rpc(server, namespace = "pureth")]
pub trait PurethApi {
    #[method(name = "query")]
    fn query(&self, request: QueryRequest) -> RpcResult<QueryResponse>;
}

#[derive(Debug)]
pub struct PurethRpc<S> {
    service: S,
}

impl<S> PurethRpc<S> {
    pub const fn new(service: S) -> Self {
        Self { service }
    }
}

impl<S: QueryHandler + Send + Sync + 'static> PurethApiServer for PurethRpc<S> {
    fn query(&self, request: QueryRequest) -> RpcResult<QueryResponse> {
        self.service.query(request).map_err(rpc_error)
    }
}

fn rpc_error(error: QueryError) -> ErrorObjectOwned {
    match error {
        QueryError::Provider(LookupError::UnknownBlock) => ErrorObjectOwned::owned(
            EthRpcErrorCode::ResourceNotFound.code(),
            "requested data unavailable",
            None::<()>,
        ),
        QueryError::Acquisition(error) if error.is_unavailable() => ErrorObjectOwned::owned(
            EthRpcErrorCode::ResourceNotFound.code(),
            "requested data unavailable",
            None::<()>,
        ),
        QueryError::UnsupportedObject |
        QueryError::UnsupportedSchema |
        QueryError::ProofRequired |
        QueryError::InvalidPath(_) |
        QueryError::UnsupportedPath(_) |
        QueryError::Provider(_) |
        QueryError::Proof(crate::ProofAccessError::Resolution(_)) => {
            ErrorObjectOwned::from(ErrorCode::InvalidParams)
        }
        QueryError::Proof(_) | QueryError::InvalidResponse(_) | QueryError::Acquisition(_) => {
            ErrorObjectOwned::from(ErrorCode::InternalError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryService;
    use alloy_primitives::B256;
    use jsonrpsee::core::server::Methods;
    use reth_provider::ProviderError;
    use reth_pureth_receipt::{HistoricalAcquisitionError, SINGLETON_BLOCK_HASH};

    fn module() -> Methods {
        PurethRpc::new(QueryService::new().unwrap()).into_rpc().into()
    }

    #[tokio::test]
    async fn rpc_response_verifies_with_client_inputs() {
        let module = module();
        assert!(module.method("pureth_query").is_some());

        let request = QueryRequest {
            block_hash: SINGLETON_BLOCK_HASH,
            object: "receipts".to_owned(),
            schema_id: crate::SCHEMA_ID.to_owned(),
            path: "[0].logs[0].address".to_owned(),
            include_proof: true,
        };
        let rpc_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "pureth_query",
            "params": { "request": &request }
        });
        let (rpc_response, _) = module.raw_json_request(&rpc_request.to_string(), 1).await.unwrap();
        let rpc_response: serde_json::Value = serde_json::from_str(rpc_response.get()).unwrap();
        assert!(rpc_response["result"].get("gindex").is_none());
        let response: QueryResponse =
            serde_json::from_value(rpc_response["result"].clone()).unwrap();
        let known_root = alloy_primitives::b256!(
            "5036e5a260a45255df46094662d27826bb1f417e3dccb4b3a4fc313876cd4e33"
        );
        assert_eq!(response.block_hash, request.block_hash);
        assert_eq!(response.object, request.object);
        assert_eq!(response.schema_id, request.schema_id);
        assert_eq!(response.path, request.path);
        assert_eq!(response.root, known_root);
        assert_eq!(response.proof_format, "merkle_branch_v0");
        crate::verify_receipt_log_address(
            &request.schema_id,
            &request.path,
            response.value_ssz.as_ref(),
            &response.proof,
            known_root,
        )
        .unwrap();

        let mut changed_proof = response.proof.clone();
        changed_proof[0][0] ^= 1;
        assert!(crate::verify_receipt_log_address(
            &request.schema_id,
            &request.path,
            response.value_ssz.as_ref(),
            &changed_proof,
            known_root,
        )
        .is_err());
    }

    #[tokio::test]
    async fn rpc_separates_invalid_requests_from_unavailable_blocks() {
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

        assert_eq!(response["error"]["code"], -32001);
    }

    #[test]
    fn rpc_maps_unavailable_acquisition_data_to_resource_not_found() {
        for error in [
            HistoricalAcquisitionError::BlockUnavailable,
            HistoricalAcquisitionError::ReceiptsUnavailable,
            HistoricalAcquisitionError::BlockRead(ProviderError::BlockExpired {
                requested: 1,
                earliest_available: 2,
            }),
            HistoricalAcquisitionError::BlockRead(ProviderError::BlockHashNotFound(B256::ZERO)),
            HistoricalAcquisitionError::BlockRead(ProviderError::UnknownBlockHash(B256::ZERO)),
            HistoricalAcquisitionError::BlockRead(ProviderError::HeaderNotFound(B256::ZERO.into())),
            HistoricalAcquisitionError::ReceiptsRead(ProviderError::BlockExpired {
                requested: 1,
                earliest_available: 2,
            }),
            HistoricalAcquisitionError::ReceiptsRead(ProviderError::ReceiptNotFound(
                B256::ZERO.into(),
            )),
        ] {
            assert_eq!(
                rpc_error(QueryError::Acquisition(error)).code(),
                EthRpcErrorCode::ResourceNotFound.code()
            );
        }
    }

    #[test]
    fn rpc_maps_acquisition_failures_to_internal_error() {
        for error in [
            HistoricalAcquisitionError::BlockRead(ProviderError::InvalidStorageOutput),
            HistoricalAcquisitionError::ConsistentView(ProviderError::BlockExpired {
                requested: 1,
                earliest_available: 2,
            }),
            HistoricalAcquisitionError::CanonicalHashRead(ProviderError::HeaderNotFound(
                B256::ZERO.into(),
            )),
        ] {
            assert_eq!(rpc_error(QueryError::Acquisition(error)).code(), -32603);
        }
    }
}
