use crate::{
    QueryError, QueryHandler, QueryRequest, QueryResponse, SelectionError, SelectionExecutor,
    SelectionLimits,
};
use alloy_rpc_types_eth::error::EthRpcErrorCode;
use jsonrpsee::{
    core::{server::RpcModule, RpcResult},
    proc_macros::rpc,
    types::{ErrorCode, ErrorObjectOwned},
};
use reth_pureth_receipt::LookupError;
use std::{sync::Arc, time::Duration};

#[rpc(server, namespace = "pureth")]
pub trait PurethApi {
    #[method(name = "query")]
    async fn query(&self, request: QueryRequest) -> RpcResult<QueryResponse>;
}

#[derive(Debug)]
pub struct PurethRpc<S> {
    service: Arc<S>,
    executor: SelectionExecutor,
}

impl<S> PurethRpc<S> {
    pub fn new(service: S) -> Self {
        Self { service: Arc::new(service), executor: SelectionExecutor::default() }
    }
}

#[jsonrpsee::core::async_trait]
impl<S: QueryHandler + Send + Sync + 'static> PurethApiServer for PurethRpc<S> {
    fn into_rpc(self) -> RpcModule<Self> {
        let mut module = RpcModule::new(self);
        module
            .register_async_method("pureth_query", |params, context, _| async move {
                if params.len_bytes() > SelectionLimits::default().request_bytes {
                    return Err(rpc_error(QueryError::Selection(SelectionError::LimitExceeded(
                        "request bytes",
                    ))));
                }
                let service = Arc::clone(&context.service);
                context
                    .executor
                    .run(Duration::from_secs(30), move |cancelled| {
                        let request = if params.is_object() {
                            params.parse::<QueryParams>().map(|params| params.request)
                        } else {
                            params.one::<QueryRequest>()
                        }
                        .map_err(|_| QueryError::Selection(SelectionError::InvalidValue))?;
                        service.query_with_cancel(request, cancelled)
                    })
                    .await
                    .map_err(rpc_error)
            })
            .expect("the query method is registered once");
        module
    }

    async fn query(&self, request: QueryRequest) -> RpcResult<QueryResponse> {
        let service = Arc::clone(&self.service);
        self.executor
            .run(Duration::from_secs(30), move |cancelled| {
                service.query_with_cancel(request, cancelled)
            })
            .await
            .map_err(rpc_error)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryParams {
    request: QueryRequest,
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
        QueryError::Selection(SelectionError::Deadline | SelectionError::Cancelled) => {
            ErrorObjectOwned::owned(-32000, "query deadline or cancellation", None::<()>)
        }
        QueryError::Selection(SelectionError::ExecutionFailed | SelectionError::InvalidProof) |
        QueryError::InvalidSnapshot |
        QueryError::Acquisition(_) => ErrorObjectOwned::from(ErrorCode::InternalError),
        QueryError::UnsupportedObject | QueryError::Provider(_) | QueryError::Selection(_) => {
            ErrorObjectOwned::from(ErrorCode::InvalidParams)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        verify_query_response, QueryService, ReceiptSelection, SelectionLimits, SelectionOperation,
        SelectionRequest,
    };
    use alloy_primitives::B256;
    use jsonrpsee::core::server::Methods;
    use reth_provider::ProviderError;
    use reth_pureth_receipt::{
        HistoricalAcquisitionError, MULTIPLE_LOGS_BLOCK_HASH, SINGLETON_BLOCK_HASH,
    };
    use std::{
        io::{Read, Write},
        net::TcpStream,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    fn module() -> Methods {
        PurethRpc::new(QueryService::new().unwrap()).into_rpc().into()
    }

    fn request(block_hash: B256) -> QueryRequest {
        QueryRequest {
            block_hash,
            object: "receipts".to_owned(),
            selection: SelectionRequest {
                selections: vec![
                    ReceiptSelection {
                        path: "[0].status".to_owned(),
                        operation: SelectionOperation::Value {},
                    },
                    ReceiptSelection {
                        path: "[0].logs[0].topics[0]".to_owned(),
                        operation: SelectionOperation::Value {},
                    },
                    ReceiptSelection {
                        path: "[0].logs[0].data".to_owned(),
                        operation: SelectionOperation::Slice { start: 1, end: 3 },
                    },
                ],
                include_proof: true,
            },
        }
    }

    async fn call(module: &Methods, request: serde_json::Value) -> serde_json::Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "pureth_query",
            "params": { "request": request }
        });
        let (response, _) = module.raw_json_request(&request.to_string(), 1).await.unwrap();
        serde_json::from_str(response.get()).unwrap()
    }

    #[tokio::test]
    async fn installed_rpc_returns_eip_selections_without_runtime_identifiers() {
        let module = module();
        assert!(module.method("pureth_query").is_some());
        let request = request(SINGLETON_BLOCK_HASH);
        let result = call(&module, serde_json::to_value(&request).unwrap()).await;
        let response: QueryResponse = serde_json::from_value(result["result"].clone()).unwrap();
        let root = response.selection.root;
        verify_query_response(&request, &response, root, SelectionLimits::default()).unwrap();
        assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[1]);
        assert_eq!(response.selection.results[1].values_ssz[0].as_ref(), &[0x22; 32]);
        assert_eq!(response.selection.results[2].values_ssz[0].as_ref(), &[0x02, 0x03]);
        let json = serde_json::to_string(&response).unwrap();
        for obsolete in ["schema_id", "producer_revision", "gindex", "merkle_branch_v0"] {
            assert!(!json.contains(obsolete));
        }
        let mut changed = response;
        changed.selection.results[0].witnesses.as_mut().unwrap()[0].branch[0][0] ^= 1;
        assert!(
            verify_query_response(&request, &changed, root, SelectionLimits::default()).is_err()
        );
    }

    #[tokio::test]
    async fn rpc_rejects_obsolete_wire_fields_and_distinguishes_missing_blocks() {
        let module = module();
        for field in ["schema_id", "producer_revision", "path", "include_proof"] {
            let mut json = serde_json::to_value(request(SINGLETON_BLOCK_HASH)).unwrap();
            json[field] = serde_json::json!("obsolete");
            assert_eq!(call(&module, json).await["error"]["code"], -32602);
        }
        assert_eq!(
            call(&module, serde_json::to_value(request(B256::repeat_byte(0xff))).unwrap()).await
                ["error"]["code"],
            -32001
        );
        let mut invalid = request(SINGLETON_BLOCK_HASH);
        invalid.selection.selections[0].path = "[9].status".to_owned();
        assert_eq!(
            call(&module, serde_json::to_value(invalid).unwrap()).await["error"]["code"],
            -32602
        );
    }

    #[tokio::test]
    async fn value_only_rpc_is_explicitly_unverified() {
        let module = module();
        let mut request = request(MULTIPLE_LOGS_BLOCK_HASH);
        request.selection.selections[0].path = "[0].logs[1].address".to_owned();
        request.selection.include_proof = false;
        let result = call(&module, serde_json::to_value(&request).unwrap()).await;
        let response: QueryResponse = serde_json::from_value(result["result"].clone()).unwrap();
        assert!(response.selection.proof_format.is_none());
        assert_eq!(response.selection.results[0].values_ssz[0].as_ref(), &[0x33; 20]);
        assert!(response.selection.results.iter().all(|result| result.witnesses.is_none()));
        assert!(verify_query_response(
            &request,
            &response,
            response.selection.root,
            SelectionLimits::default()
        )
        .is_err());
    }

    #[test]
    fn source_error_mapping_preserves_stage_and_missing_data() {
        for error in [
            HistoricalAcquisitionError::BlockUnavailable,
            HistoricalAcquisitionError::ReceiptsUnavailable,
            HistoricalAcquisitionError::Snapshot(
                reth_pureth_receipt::Eip6466SnapshotError::MissingData(
                    reth_pureth_receipt::ReceiptConstructionError::MissingAuthorizationOutcomes {
                        transaction_hash: B256::ZERO,
                    },
                ),
            ),
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
            assert_eq!(rpc_error(QueryError::Acquisition(error)).code(), -32001);
        }
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
        assert_eq!(rpc_error(QueryError::Selection(SelectionError::Deadline)).code(), -32000);
    }

    struct CountingHandler {
        calls: Arc<AtomicUsize>,
        service: QueryService,
    }

    impl QueryHandler for CountingHandler {
        fn query_with_cancel(
            &self,
            request: QueryRequest,
            cancelled: &AtomicBool,
        ) -> Result<QueryResponse, QueryError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.service.query_with_cancel(request, cancelled)
        }
    }

    #[tokio::test]
    async fn installed_rpc_bounds_raw_named_and_positional_params_before_dispatch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let module: Methods = PurethRpc::new(CountingHandler {
            calls: Arc::clone(&calls),
            service: QueryService::new().unwrap(),
        })
        .into_rpc()
        .into();
        let json = serde_json::to_string(&request(SINGLETON_BLOCK_HASH)).unwrap();
        for named in [false, true] {
            for length in [65_535, 65_536, 65_537, 70_000] {
                let wrapper =
                    if named { format!("{{\"request\":{json}}}") } else { format!("[{json}]") };
                let params = format!(
                    "{}{}{}",
                    &wrapper[..1],
                    " ".repeat(length - wrapper.len()),
                    &wrapper[1..]
                );
                assert_eq!(params.len(), length);
                let raw = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"pureth_query\",\"params\":{params}}}");
                let before = calls.load(Ordering::Relaxed);
                let (response, _) = module.raw_json_request(&raw, 1).await.unwrap();
                let response: serde_json::Value = serde_json::from_str(response.get()).unwrap();
                if length <= 65_536 {
                    assert!(response.get("result").is_some());
                    assert_eq!(calls.load(Ordering::Relaxed), before + 1);
                } else {
                    assert_eq!(response["error"]["code"], -32602);
                    assert_eq!(calls.load(Ordering::Relaxed), before);
                }
            }
        }
        for params in [
            format!("[{json},{json}]"),
            format!("{{\"request\":{json},\"extra\":1}}"),
            format!(
                "{{\"request\":{{\"selection\":{{\"selections\":[{}]}}}}}}",
                "{},".repeat(23_000).trim_end_matches(',')
            ),
        ] {
            let raw = format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"pureth_query\",\"params\":{params}}}"
            );
            let before = calls.load(Ordering::Relaxed);
            let (response, _) = module.raw_json_request(&raw, 1).await.unwrap();
            let response: serde_json::Value = serde_json::from_str(response.get()).unwrap();
            assert_eq!(response["error"]["code"], -32602);
            assert_eq!(calls.load(Ordering::Relaxed), before);
        }
    }

    #[tokio::test]
    async fn http_rpc_rejects_oversized_raw_params_before_query_handler() {
        let calls = Arc::new(AtomicUsize::new(0));
        let server =
            jsonrpsee::server::ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let handle = server.start(
            PurethRpc::new(CountingHandler {
                calls: Arc::clone(&calls),
                service: QueryService::new().unwrap(),
            })
            .into_rpc(),
        );
        let json = serde_json::to_string(&request(SINGLETON_BLOCK_HASH)).unwrap();
        let raw = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"pureth_query\",\"params\":{{{}\"request\":{json}}}}}", " ".repeat(70_000));
        let response = tokio::task::spawn_blocking(move || {
            let mut socket = TcpStream::connect(address).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            write!(socket, "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", raw.len(), raw).unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            response
        }).await.unwrap();
        handle.stop().unwrap();
        handle.stopped().await;
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["error"]["code"], -32602);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }
}
