#![allow(unreachable_pub)]
//! Standalone http tests

use crate::utils::{launch_http, launch_http_ws, launch_ws};
use alloy_eips::{eip1898::LenientBlockNumberOrTag, BlockId, BlockNumberOrTag};
use alloy_json_rpc::RpcSend;
use alloy_primitives::{bytes, Address, Bytes, TxHash, B256, B64, U256, U64};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_eth::{
    transaction::TransactionRequest, Account, Block, EIP1186AccountProofResponse, FeeHistory,
    Filter, FilterChanges, FilterId, Index, Log, PendingTransactionFilterKind, SyncStatus,
    Transaction, TransactionReceipt,
};
use alloy_rpc_types_mev::EthCallBundle;
use alloy_rpc_types_trace::{filter::TraceFilter, parity::TraceType};
use alloy_transport::TransportError;
use jsonrpsee::types::error::ErrorCode;
use reth_network_peers::NodeRecord;
use reth_rpc_server_types::RethRpcModule;
use reth_trie_common::ExecutionWitnessMode;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Builds the positional JSON-RPC params of a request.
macro_rules! rpc_params {
    ($($param:expr),* $(,)?) => {
        serde_json::json!([$($param),*])
    };
}

fn is_unimplemented(err: TransportError) -> bool {
    err.as_error_resp().is_some_and(|payload| {
        payload.code == i64::from(ErrorCode::InternalError.code()) &&
            payload.message == "unimplemented"
    })
}

fn is_invalid_params(err: &TransportError) -> bool {
    err.as_error_resp()
        .is_some_and(|payload| payload.code == i64::from(ErrorCode::InvalidParams.code()))
}

async fn test_rpc_call_ok<R>(client: &RpcClient, method_name: &'static str, params: impl RpcSend)
where
    R: DeserializeOwned + std::fmt::Debug + Send + Sync + Unpin + 'static,
{
    // Make the RPC request
    match client.request::<_, R>(method_name, params).await {
        Ok(_) => {} // If the request is successful, do nothing
        Err(e) => {
            // If an error occurs, panic with the error message
            panic!("Expected successful response, got error: {e:?}");
        }
    }
}

async fn test_rpc_call_err<R>(client: &RpcClient, method_name: &'static str, params: impl RpcSend)
where
    R: DeserializeOwned + std::fmt::Debug + Send + Sync + Unpin + 'static,
{
    // Make the RPC request
    if let Ok(resp) = client.request::<_, R>(method_name, params).await {
        // Panic if an unexpected successful response is received
        panic!("Expected error response, got successful response: {resp:?}");
    };
}

async fn assert_empty_raw_transaction_sync_error(client: &RpcClient, params: impl RpcSend) {
    let err = client
        .request::<_, Value>("eth_sendRawTransactionSync", params)
        .await
        .expect_err("empty raw transaction should fail after RPC parameter decoding");
    assert!(err.to_string().contains("empty transaction data"), "{err:?}");
}

/// Represents a builder for creating JSON-RPC requests.
#[derive(Clone, Serialize, Deserialize)]
pub struct RawRpcParamsBuilder {
    method: Option<String>,
    params: Vec<Value>,
    id: i32,
}

impl RawRpcParamsBuilder {
    /// Sets the method name for the JSON-RPC request.
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    /// Adds a parameter to the JSON-RPC request.
    pub fn add_param<S: Serialize>(mut self, param: S) -> Self {
        self.params.push(serde_json::to_value(param).expect("Failed to serialize parameter"));
        self
    }

    /// Sets the ID for the JSON-RPC request.
    pub const fn set_id(mut self, id: i32) -> Self {
        self.id = id;
        self
    }

    /// Constructs the JSON-RPC request string based on the provided configurations.
    pub fn build(self) -> String {
        let Self { method, params, id } = self;
        let method = method.unwrap_or_else(|| panic!("JSON-RPC method not set"));
        let params: Vec<String> = params.into_iter().map(|p| p.to_string()).collect();

        format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"{}","params":[{}]}}"#,
            id,
            method,
            params.join(",")
        )
    }
}

impl Default for RawRpcParamsBuilder {
    fn default() -> Self {
        Self { method: None, params: Vec::new(), id: 1 }
    }
}

async fn test_filter_calls(client: &RpcClient) {
    client.request::<_, FilterId>("eth_newFilter", (Filter::default(),)).await.unwrap();
    client
        .request::<_, FilterId>(
            "eth_newPendingTransactionFilter",
            (None::<PendingTransactionFilterKind>,),
        )
        .await
        .unwrap();
    client
        .request::<_, FilterId>(
            "eth_newPendingTransactionFilter",
            (Some(PendingTransactionFilterKind::Full),),
        )
        .await
        .unwrap();
    let id = client.request_noparams::<FilterId>("eth_newBlockFilter").await.unwrap();
    client.request::<_, FilterChanges>("eth_getFilterChanges", (id.clone(),)).await.unwrap();
    client.request::<_, Vec<Log>>("eth_getLogs", (Filter::default(),)).await.unwrap();
    let id = client.request::<_, FilterId>("eth_newFilter", (Filter::default(),)).await.unwrap();
    client.request::<_, Vec<Log>>("eth_getFilterLogs", (id.clone(),)).await.unwrap();
    client.request::<_, bool>("eth_uninstallFilter", (id,)).await.unwrap();
}

async fn test_basic_admin_calls(client: &RpcClient) {
    let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
    let node: NodeRecord = url.parse().unwrap();

    client.request::<_, bool>("admin_addPeer", (node,)).await.unwrap();
    client.request::<_, bool>("admin_removePeer", (node,)).await.unwrap();
    client.request::<_, bool>("admin_addTrustedPeer", (node,)).await.unwrap();
    client.request::<_, bool>("admin_removeTrustedPeer", (node,)).await.unwrap();
    client.request::<_, bool>("admin_banPeer", (node,)).await.unwrap();
    client.request::<_, bool>("admin_unbanPeer", (node,)).await.unwrap();
    client.request_noparams::<Value>("admin_nodeInfo").await.unwrap();
}

async fn test_basic_eth_calls(client: &RpcClient) {
    let address = Address::default();
    let index = Index::default();
    let hash = B256::default();
    let tx_hash = TxHash::default();
    let block_number = BlockNumberOrTag::default();
    let call_request = TransactionRequest::default();
    let transaction_request = TransactionRequest::default();
    let bytes = Bytes::default();
    let tx = bytes!("02f871018303579880850555633d1b82520894eee27662c2b8eba3cd936a23f039f3189633e4c887ad591c62bdaeb180c080a07ea72c68abfb8fca1bd964f0f99132ed9280261bdca3e549546c0205e800f7d0a05b4ef3039e9c9b9babc179a1878fb825b5aaf5aed2fa8744854150157b08d6f3");
    let typed_data = serde_json::from_str::<Value>(
        r#"{
        "types": {
            "EIP712Domain": []
        },
        "primaryType": "EIP712Domain",
        "domain": {},
        "message": {}
    }"#,
    )
    .unwrap();

    // Implemented
    client.request_noparams::<U64>("eth_protocolVersion").await.unwrap();
    client.request_noparams::<Option<U64>>("eth_chainId").await.unwrap();
    client.request_noparams::<Vec<Address>>("eth_accounts").await.unwrap();
    client
        .request::<_, Option<Account>>("eth_getAccount", (address, BlockId::from(block_number)))
        .await
        .unwrap();
    client.request_noparams::<U256>("eth_blockNumber").await.unwrap();
    client.request::<_, Bytes>("eth_getCode", (address, None::<BlockId>)).await.unwrap();
    client.request::<_, B256>("eth_sendRawTransaction", (tx,)).await.unwrap();
    client
        .request::<_, FeeHistory>("eth_feeHistory", (U64::from(0), block_number, None::<Vec<f64>>))
        .await
        .unwrap();
    client.request::<_, U256>("eth_getBalance", (address, None::<BlockId>)).await.unwrap();
    client.request::<_, U256>("eth_getTransactionCount", (address, None::<BlockId>)).await.unwrap();
    client
        .request::<_, B256>("eth_getStorageAt", (address, U256::default(), None::<BlockId>))
        .await
        .unwrap();
    client.request::<_, Option<Block>>("eth_getBlockByHash", (hash, false)).await.unwrap();
    client
        .request::<_, Option<Block>>("eth_getBlockByNumber", (block_number, false))
        .await
        .unwrap();
    client
        .request::<_, Option<U256>>("eth_getBlockTransactionCountByNumber", (block_number,))
        .await
        .unwrap();
    client.request::<_, Option<U256>>("eth_getBlockTransactionCountByHash", (hash,)).await.unwrap();
    client.request::<_, Option<U256>>("eth_getUncleCountByBlockHash", (hash,)).await.unwrap();
    client
        .request::<_, Option<U256>>("eth_getUncleCountByBlockNumber", (block_number,))
        .await
        .unwrap();
    client
        .request::<_, Option<Block>>("eth_getUncleByBlockHashAndIndex", (hash, index))
        .await
        .unwrap();
    client
        .request::<_, Option<Block>>("eth_getUncleByBlockNumberAndIndex", (block_number, index))
        .await
        .unwrap();
    client.request::<_, Bytes>("eth_sign", (address, bytes.clone())).await.unwrap_err();
    client.request::<_, Bytes>("eth_signTypedData", (address, typed_data)).await.unwrap_err();
    client.request::<_, Option<Transaction>>("eth_getTransactionByHash", (tx_hash,)).await.unwrap();
    client
        .request::<_, Option<Transaction>>("eth_getTransactionByBlockHashAndIndex", (hash, index))
        .await
        .unwrap();
    client
        .request::<_, Option<Transaction>>(
            "eth_getTransactionByBlockNumberAndIndex",
            (block_number, index),
        )
        .await
        .unwrap();
    client
        .request::<_, Value>(
            "eth_createAccessList",
            (call_request.clone(), Some(BlockId::from(block_number)), Value::Null),
        )
        .await
        .unwrap_err();
    client
        .request::<_, U256>(
            "eth_estimateGas",
            (call_request.clone(), Some(BlockId::from(block_number)), Value::Null, Value::Null),
        )
        .await
        .unwrap_err();
    client
        .request::<_, Bytes>(
            "eth_call",
            (call_request.clone(), Some(BlockId::from(block_number)), Value::Null, Value::Null),
        )
        .await
        .unwrap_err();
    client.request_noparams::<SyncStatus>("eth_syncing").await.unwrap();
    client
        .request::<_, B256>("eth_sendTransaction", (transaction_request.clone(),))
        .await
        .unwrap_err();
    client.request::<_, Bytes>("eth_signTransaction", (transaction_request,)).await.unwrap_err();
    client.request_noparams::<U256>("eth_hashrate").await.unwrap();
    client
        .request::<_, bool>("eth_submitHashrate", (U256::default(), B256::default()))
        .await
        .unwrap();
    client.request_noparams::<U256>("eth_gasPrice").await.unwrap_err();
    client.request_noparams::<U256>("eth_maxPriorityFeePerGas").await.unwrap_err();
    client
        .request::<_, EIP1186AccountProofResponse>(
            "eth_getProof",
            (address, Vec::<B256>::new(), None::<BlockId>),
        )
        .await
        .unwrap();
    let proofs = client
        .request::<_, Vec<EIP1186AccountProofResponse>>(
            "eth_getMultiProof",
            (vec![(address, vec![B256::ZERO])], None::<BlockId>),
        )
        .await
        .unwrap();
    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].address, address);

    // Unimplemented
    assert!(is_unimplemented(
        client.request_noparams::<Address>("eth_coinbase").await.err().unwrap()
    ));
    assert!(is_unimplemented(client.request_noparams::<bool>("eth_mining").await.err().unwrap()));
    assert!(is_unimplemented(client.request_noparams::<Value>("eth_getWork").await.err().unwrap()));
    assert!(is_unimplemented(
        client
            .request::<_, bool>(
                "eth_submitWork",
                (B64::default(), B256::default(), B256::default())
            )
            .await
            .err()
            .unwrap()
    ));
    client.request::<_, Value>("eth_callBundle", (EthCallBundle::default(),)).await.unwrap_err();
}

async fn test_basic_debug_calls(client: &RpcClient) {
    let block_id = BlockId::latest();

    client.request::<_, Bytes>("debug_getRawHeader", (block_id,)).await.unwrap_err();
    client.request::<_, Bytes>("debug_getRawBlock", (block_id,)).await.unwrap_err();
    client
        .request::<_, Option<Bytes>>("debug_getRawTransaction", (B256::default(),))
        .await
        .unwrap();
    client.request::<_, Vec<Bytes>>("debug_getRawReceipts", (block_id,)).await.unwrap_err();
    client.request_noparams::<Vec<Value>>("debug_getBadBlocks").await.unwrap();
    client.request_noparams::<()>("debug_clearTxpool").await.unwrap();
    client
        .request::<_, Value>("debug_accountAt", (block_id, Index::default(), Address::default()))
        .await
        .unwrap_err();
    client
        .request::<_, Value>(
            "debug_accountInfoAt",
            (block_id, Index::default(), Address::default()),
        )
        .await
        .unwrap_err();

    for block_id in [
        BlockId::number(0),
        BlockId::latest(),
        BlockId::hash(B256::ZERO),
        BlockId::hash_canonical(B256::ZERO),
    ] {
        let err = client
            .request::<_, Value>("debug_executionWitness", (block_id, None::<ExecutionWitnessMode>))
            .await
            .unwrap_err();
        assert!(!is_invalid_params(&err));
    }

    let err = client
        .request::<_, Value>(
            "debug_executionWitnessByBlockHash",
            (B256::ZERO, Some(ExecutionWitnessMode::default())),
        )
        .await
        .unwrap_err();
    assert!(!is_invalid_params(&err));
}

async fn test_basic_net_calls(client: &RpcClient) {
    client.request_noparams::<String>("net_version").await.unwrap();
    client.request_noparams::<U64>("net_peerCount").await.unwrap();
    client.request_noparams::<bool>("net_listening").await.unwrap();
}

async fn test_basic_trace_calls(client: &RpcClient) {
    let block_id = BlockId::latest();
    let trace_filter = TraceFilter::default();

    client
        .request::<_, Value>(
            "trace_rawTransaction",
            (Bytes::default(), HashSet::<TraceType>::default(), None::<BlockId>),
        )
        .await
        .unwrap_err();
    client
        .request::<_, Value>(
            "trace_callMany",
            (Vec::<(TransactionRequest, HashSet<TraceType>)>::new(), Some(BlockId::latest())),
        )
        .await
        .unwrap_err();
    assert!(client
        .request::<_, Option<Value>>(
            "trace_replayTransaction",
            (B256::default(), HashSet::<TraceType>::default()),
        )
        .await
        .unwrap()
        .is_none());
    client.request::<_, Value>("trace_block", (block_id,)).await.unwrap_err();
    client
        .request::<_, Value>(
            "trace_replayBlockTransactions",
            (block_id, HashSet::<TraceType>::default()),
        )
        .await
        .unwrap_err();

    client.request::<_, Value>("trace_filter", (trace_filter,)).await.unwrap();
}

async fn test_basic_web3_calls(client: &RpcClient) {
    client.request_noparams::<String>("web3_clientVersion").await.unwrap();
    client.request::<_, B256>("web3_sha3", (Bytes::default(),)).await.unwrap();
}

async fn test_basic_otterscan_calls(client: &RpcClient) {
    let address = Address::default();
    let sender = Address::default();
    let tx_hash = TxHash::default();
    let block_number = 1;
    let page_number = 1;
    let page_size = 10;
    let nonce = 1;
    let block_hash = B256::default();

    client
        .request::<_, Value>(
            "ots_getHeaderByNumber",
            (LenientBlockNumberOrTag::new(BlockNumberOrTag::Number(block_number)),),
        )
        .await
        .unwrap();

    client.request::<_, bool>("ots_hasCode", (address, None::<BlockId>)).await.unwrap();
    client
        .request::<_, bool>("ots_hasCode", (address, Some(BlockId::from(block_number))))
        .await
        .unwrap();

    client.request_noparams::<u64>("ots_getApiLevel").await.unwrap();

    client.request::<_, Value>("ots_getInternalOperations", (tx_hash,)).await.unwrap();

    client.request::<_, Option<Bytes>>("ots_getTransactionError", (tx_hash,)).await.unwrap();

    client.request::<_, Value>("ots_traceTransaction", (tx_hash,)).await.unwrap();

    client
        .request::<_, Value>(
            "ots_getBlockDetails",
            (LenientBlockNumberOrTag::new(BlockNumberOrTag::Number(block_number)),),
        )
        .await
        .unwrap_err();
    client
        .request::<_, Value>("ots_getBlockDetails", (LenientBlockNumberOrTag::default(),))
        .await
        .unwrap_err();

    client.request::<_, Value>("ots_getBlockDetailsByHash", (block_hash,)).await.unwrap_err();

    client
        .request::<_, Value>(
            "ots_getBlockTransactions",
            (
                LenientBlockNumberOrTag::new(BlockNumberOrTag::Number(block_number)),
                page_number,
                page_size,
            ),
        )
        .await
        .err()
        .unwrap();

    assert!(is_unimplemented(
        client
            .request::<_, Value>(
                "ots_searchTransactionsBefore",
                (
                    address,
                    LenientBlockNumberOrTag::new(BlockNumberOrTag::Number(block_number)),
                    page_size,
                ),
            )
            .await
            .err()
            .unwrap()
    ));
    assert!(is_unimplemented(
        client
            .request::<_, Value>(
                "ots_searchTransactionsAfter",
                (
                    address,
                    LenientBlockNumberOrTag::new(BlockNumberOrTag::Number(block_number)),
                    page_size,
                ),
            )
            .await
            .err()
            .unwrap()
    ));
    assert!(client
        .request::<_, Option<TxHash>>("ots_getTransactionBySenderAndNonce", (sender, nonce))
        .await
        .err()
        .is_none());
    assert!(client
        .request::<_, Option<Value>>("ots_getContractCreator", (address,))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_filter_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();
    test_filter_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_admin_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Admin]).await;
    let client = handle.http_client().unwrap();
    test_basic_admin_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_admin_functions_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws(vec![RethRpcModule::Admin]).await;
    let client = handle.ws_client().await.unwrap();
    test_basic_admin_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_admin_functions_http_and_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Admin]).await;
    let client = handle.http_client().unwrap();
    test_basic_admin_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_eth_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();
    test_basic_eth_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_send_raw_transaction_sync_accepts_optional_timeout_arg() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    assert_empty_raw_transaction_sync_error(&client, (Bytes::default(),)).await;
    assert_empty_raw_transaction_sync_error(&client, (Bytes::default(), Value::Null)).await;
    assert_empty_raw_transaction_sync_error(&client, (Bytes::default(), 1u64)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_eth_functions_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws(vec![RethRpcModule::Eth]).await;
    let client = handle.ws_client().await.unwrap();
    test_basic_eth_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_eth_functions_http_and_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();
    test_basic_eth_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_debug_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Debug]).await;
    let client = handle.http_client().unwrap();
    test_basic_debug_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_debug_functions_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws(vec![RethRpcModule::Debug]).await;
    let client = handle.ws_client().await.unwrap();
    test_basic_debug_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_debug_functions_http_and_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Debug]).await;
    let client = handle.http_client().unwrap();
    test_basic_debug_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_net_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Net]).await;
    let client = handle.http_client().unwrap();
    test_basic_net_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_net_functions_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws(vec![RethRpcModule::Net]).await;
    let client = handle.ws_client().await.unwrap();
    test_basic_net_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_net_functions_http_and_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Net]).await;
    let client = handle.http_client().unwrap();
    test_basic_net_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_trace_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Trace]).await;
    let client = handle.http_client().unwrap();
    test_basic_trace_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_trace_functions_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws(vec![RethRpcModule::Trace]).await;
    let client = handle.ws_client().await.unwrap();
    test_basic_trace_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_trace_functions_http_and_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Trace]).await;
    let client = handle.http_client().unwrap();
    test_basic_trace_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_web3_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Web3]).await;
    let client = handle.http_client().unwrap();
    test_basic_web3_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_web3_functions_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws(vec![RethRpcModule::Web3]).await;
    let client = handle.ws_client().await.unwrap();
    test_basic_web3_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_web3_functions_http_and_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Web3]).await;
    let client = handle.http_client().unwrap();
    test_basic_web3_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_otterscan_functions_http() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Ots]).await;
    let client = handle.http_client().unwrap();
    test_basic_otterscan_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_otterscan_functions_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws(vec![RethRpcModule::Ots]).await;
    let client = handle.ws_client().await.unwrap();
    test_basic_otterscan_calls(&client).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_call_otterscan_functions_http_and_ws() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Ots]).await;
    let client = handle.http_client().unwrap();
    test_basic_otterscan_calls(&client).await;
}

// <https://github.com/paradigmxyz/reth/issues/5830>
#[tokio::test(flavor = "multi_thread")]
async fn test_eth_logs_args() {
    reth_tracing::init_test_tracing();

    let handle = launch_http_ws(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    let params = rpc_params![
        serde_json::json!({"blockHash":"0x58dc57ab582b282c143424bd01e8d923cddfdcda9455bad02a29522f6274a948"})
    ];

    let resp = client.request::<_, Vec<Log>>("eth_getLogs", params).await;
    // block does not exist
    assert!(resp.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_block_by_number_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting block by number with proper fields
    test_rpc_call_ok::<Option<Block>>(
        &client,
        "eth_getBlockByNumber",
        rpc_params!["0x1b4", true], // Block number and full transaction object flag
    )
    .await;

    // Requesting block by number with wrong fields
    test_rpc_call_err::<Option<Block>>(
        &client,
        "eth_getBlockByNumber",
        rpc_params!["0x1b4", "0x1b4"],
    )
    .await;

    // Requesting block by number with missing fields
    test_rpc_call_err::<Option<Block>>(&client, "eth_getBlockByNumber", rpc_params!["0x1b4"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_block_by_hash_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting block by hash with proper fields
    test_rpc_call_ok::<Option<Block>>(
        &client,
        "eth_getBlockByHash",
        rpc_params!["0xdc0818cf78f21a8e70579cb46a43643f78291264dda342ae31049421c82d21ae", false],
    )
    .await;

    // Requesting block by hash with wrong fields
    test_rpc_call_err::<Option<Block>>(
        &client,
        "eth_getBlockByHash",
        rpc_params!["0xdc0818cf78f21a8e70579cb46a43643f78291264dda342ae31049421c82d21ae", "0x1b4"],
    )
    .await;

    // Requesting block by hash with missing fields
    test_rpc_call_err::<Option<Block>>(
        &client,
        "eth_getBlockByHash",
        rpc_params!["0xdc0818cf78f21a8e70579cb46a43643f78291264dda342ae31049421c82d21ae"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_code_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting code at a given address with proper fields
    test_rpc_call_ok::<Bytes>(
        &client,
        "eth_getCode",
        rpc_params![
            "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b",
            "0x2" // 2
        ],
    )
    .await;

    // Define test cases with different default block parameters
    let default_block_params = vec!["earliest", "latest", "pending"];

    // Iterate over test cases
    for param in default_block_params {
        // Requesting code at a given address with default block parameter
        test_rpc_call_ok::<Bytes>(
            &client,
            "eth_getCode",
            rpc_params!["0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b", param],
        )
        .await;
    }

    // Without block number which is optional
    test_rpc_call_ok::<Bytes>(
        &client,
        "eth_getCode",
        rpc_params!["0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b"],
    )
    .await;

    // Requesting code at a given address with invalid default block parameter
    test_rpc_call_err::<Bytes>(
        &client,
        "eth_getCode",
        rpc_params!["0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b", "finalized"],
    )
    .await;

    // Requesting code at a given address with wrong fields
    test_rpc_call_err::<Bytes>(
        &client,
        "eth_getCode",
        rpc_params!["0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b", false],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_block_number_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting block number without any parameter
    test_rpc_call_ok::<U256>(&client, "eth_blockNumber", rpc_params![]).await;

    // Define test cases with different default block parameters
    let invalid_default_block_params = vec!["finalized", "0x2"];

    // Iterate over test cases
    for param in invalid_default_block_params {
        // Requesting block number with invalid parameter should not throw an error
        test_rpc_call_ok::<U256>(&client, "eth_blockNumber", rpc_params![param]).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_chain_id_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting chain ID without any parameter
    test_rpc_call_ok::<Option<U64>>(&client, "eth_chainId", rpc_params![]).await;

    // Define test cases with different invalid parameters
    let invalid_params = vec!["finalized", "0x2"];

    // Iterate over test cases
    for param in invalid_params {
        // Requesting chain ID with invalid parameter should not throw an error
        test_rpc_call_ok::<Option<U64>>(&client, "eth_chainId", rpc_params![param]).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_syncing_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting syncing status
    test_rpc_call_ok::<Option<SyncStatus>>(&client, "eth_syncing", rpc_params![]).await;

    // Define test cases with invalid parameters
    let invalid_params = vec!["latest", "earliest", "pending", "0x2"];

    // Iterate over test cases
    for param in invalid_params {
        // Requesting syncing status with invalid parameter should not throw an error
        test_rpc_call_ok::<Option<SyncStatus>>(&client, "eth_syncing", rpc_params![param]).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_protocol_version_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting protocol version without any parameter
    test_rpc_call_ok::<U64>(&client, "eth_protocolVersion", rpc_params![]).await;

    // Define test cases with invalid parameters
    let invalid_params = vec!["latest", "earliest", "pending", "0x2"];

    // Iterate over test cases
    for param in invalid_params {
        // Requesting protocol version with invalid parameter should not throw an error
        test_rpc_call_ok::<U64>(&client, "eth_protocolVersion", rpc_params![param]).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_coinbase_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting coinbase address without any parameter should return Unimplemented
    match client.request::<_, Address>("eth_coinbase", rpc_params![]).await {
        Ok(_) => {
            // If there's a response, it's unexpected, panic
            panic!("Expected Unimplemented error, got successful response");
        }
        Err(err) => {
            assert!(is_unimplemented(err));
        }
    };
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_accounts_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting accounts without any parameter
    test_rpc_call_ok::<Option<Vec<Address>>>(&client, "eth_accounts", rpc_params![]).await;

    // Define test cases with invalid parameters
    let invalid_params = vec!["latest", "earliest", "pending", "0x2"];

    // Iterate over test cases
    for param in invalid_params {
        // Requesting accounts with invalid parameter should not throw an error
        test_rpc_call_ok::<Option<Vec<Address>>>(&client, "eth_accounts", rpc_params![param]).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_block_transaction_count_by_hash_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting transaction count by block hash with proper fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getBlockTransactionCountByHash",
        rpc_params!["0xb903239f8543d04b5dc1ba6579132b143087c68db1b2168786408fcbce568238"],
    )
    .await;

    // Requesting transaction count by block hash with additional fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getBlockTransactionCountByHash",
        rpc_params!["0xb903239f8543d04b5dc1ba6579132b143087c68db1b2168786408fcbce568238", true],
    )
    .await;

    // Requesting transaction count by block hash with missing fields
    test_rpc_call_err::<Option<U256>>(&client, "eth_getBlockTransactionCountByHash", rpc_params![])
        .await;

    // Requesting transaction count by block hash with wrong field
    test_rpc_call_err::<Option<U256>>(
        &client,
        "eth_getBlockTransactionCountByHash",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_block_transaction_count_by_number_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting transaction count by block number with proper fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getBlockTransactionCountByNumber",
        rpc_params!["0xe8"],
    )
    .await;

    // Requesting transaction count by block number with additional fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getBlockTransactionCountByNumber",
        rpc_params!["0xe8", true],
    )
    .await;

    // Requesting transaction count by block number with missing fields
    test_rpc_call_err::<Option<U256>>(
        &client,
        "eth_getBlockTransactionCountByNumber",
        rpc_params![],
    )
    .await;

    // Requesting transaction count by block number with wrong field
    test_rpc_call_err::<Option<U256>>(
        &client,
        "eth_getBlockTransactionCountByNumber",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_uncle_count_by_block_hash_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting uncle count by block hash with proper fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getUncleCountByBlockHash",
        rpc_params!["0xb903239f8543d04b5dc1ba6579132b143087c68db1b2168786408fcbce568238"],
    )
    .await;

    // Requesting uncle count by block hash with additional fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getUncleCountByBlockHash",
        rpc_params!["0xb903239f8543d04b5dc1ba6579132b143087c68db1b2168786408fcbce568238", true],
    )
    .await;

    // Requesting uncle count by block hash with missing fields
    test_rpc_call_err::<Option<U256>>(&client, "eth_getUncleCountByBlockHash", rpc_params![]).await;

    // Requesting uncle count by block hash with wrong field
    test_rpc_call_err::<Option<U256>>(&client, "eth_getUncleCountByBlockHash", rpc_params![true])
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_uncle_count_by_block_number_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting uncle count by block number with proper fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getUncleCountByBlockNumber",
        rpc_params!["0xe8"],
    )
    .await;

    // Requesting uncle count by block number with additional fields
    test_rpc_call_ok::<Option<U256>>(
        &client,
        "eth_getUncleCountByBlockNumber",
        rpc_params!["0xe8", true],
    )
    .await;

    // Requesting uncle count by block number with missing fields
    test_rpc_call_err::<Option<U256>>(&client, "eth_getUncleCountByBlockNumber", rpc_params![])
        .await;

    // Requesting uncle count by block number with wrong field
    test_rpc_call_err::<Option<U256>>(&client, "eth_getUncleCountByBlockNumber", rpc_params![true])
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_block_receipts_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting block receipts by block hash with proper fields
    test_rpc_call_ok::<Option<Vec<TransactionReceipt>>>(
        &client,
        "eth_getBlockReceipts",
        rpc_params!["0xe8"],
    )
    .await;

    // Requesting block receipts by block hash with additional fields
    test_rpc_call_ok::<Option<Vec<TransactionReceipt>>>(
        &client,
        "eth_getBlockReceipts",
        rpc_params!["0xe8", true],
    )
    .await;

    // Requesting block receipts by block hash with missing fields
    test_rpc_call_err::<Option<Vec<TransactionReceipt>>>(
        &client,
        "eth_getBlockReceipts",
        rpc_params![],
    )
    .await;

    // Requesting block receipts by block hash with wrong field
    test_rpc_call_err::<Option<Vec<TransactionReceipt>>>(
        &client,
        "eth_getBlockReceipts",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_uncle_by_block_hash_and_index_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting uncle by block hash and index with proper fields
    test_rpc_call_ok::<Option<Block>>(
        &client,
        "eth_getUncleByBlockHashAndIndex",
        rpc_params!["0xc6ef2fc5426d6ad6fd9e2a26abeab0aa2411b7ab17f30a99d3cb96aed1d1055b", "0x0"],
    )
    .await;

    // Requesting uncle by block hash and index with additional fields
    test_rpc_call_ok::<Option<Block>>(
        &client,
        "eth_getUncleByBlockHashAndIndex",
        rpc_params![
            "0xc6ef2fc5426d6ad6fd9e2a26abeab0aa2411b7ab17f30a99d3cb96aed1d1055b",
            "0x0",
            true
        ],
    )
    .await;

    // Requesting uncle by block hash and index with missing fields
    test_rpc_call_err::<Option<Block>>(&client, "eth_getUncleByBlockHashAndIndex", rpc_params![])
        .await;

    // Requesting uncle by block hash and index with wrong fields
    test_rpc_call_err::<Option<Block>>(
        &client,
        "eth_getUncleByBlockHashAndIndex",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_uncle_by_block_number_and_index_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting uncle by block number and index with proper fields
    test_rpc_call_ok::<Option<Block>>(
        &client,
        "eth_getUncleByBlockNumberAndIndex",
        rpc_params!["0x29c", "0x0"],
    )
    .await;

    // Requesting uncle by block number and index with additional fields
    test_rpc_call_ok::<Option<Block>>(
        &client,
        "eth_getUncleByBlockNumberAndIndex",
        rpc_params!["0x29c", "0x0", true],
    )
    .await;

    // Requesting uncle by block number and index with missing fields
    test_rpc_call_err::<Option<Block>>(&client, "eth_getUncleByBlockNumberAndIndex", rpc_params![])
        .await;

    // Requesting uncle by block number and index with wrong fields
    test_rpc_call_err::<Option<Block>>(
        &client,
        "eth_getUncleByBlockNumberAndIndex",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_transaction_by_hash_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting transaction by hash with proper fields
    test_rpc_call_ok::<Option<Transaction>>(
        &client,
        "eth_getTransactionByHash",
        rpc_params!["0x88df016429689c079f3b2f6ad39fa052532c56795b733da78a91ebe6a713944b"],
    )
    .await;

    // Requesting transaction by hash with additional fields
    test_rpc_call_ok::<Option<Transaction>>(
        &client,
        "eth_getTransactionByHash",
        rpc_params!["0x88df016429689c079f3b2f6ad39fa052532c56795b733da78a91ebe6a713944b", true],
    )
    .await;

    // Requesting transaction by hash with missing fields
    test_rpc_call_err::<Option<Transaction>>(&client, "eth_getTransactionByHash", rpc_params![])
        .await;

    // Requesting transaction by hash with wrong fields
    test_rpc_call_err::<Option<Transaction>>(
        &client,
        "eth_getTransactionByHash",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_transaction_by_block_hash_and_index_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting transaction by block hash and index with proper fields
    test_rpc_call_ok::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockHashAndIndex",
        rpc_params!["0x1dad0df7c027bcc86d06b3a6709ff78decd732c37b73123453ba7d9463eae60d", "0x0"],
    )
    .await;

    // Requesting transaction by block hash and index with additional fields
    test_rpc_call_ok::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockHashAndIndex",
        rpc_params![
            "0x1dad0df7c027bcc86d06b3a6709ff78decd732c37b73123453ba7d9463eae60d",
            "0x0",
            true
        ],
    )
    .await;

    // Requesting transaction by block hash and index with missing fields
    test_rpc_call_err::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockHashAndIndex",
        rpc_params![],
    )
    .await;

    // Requesting transaction by block hash and index with wrong fields
    test_rpc_call_err::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockHashAndIndex",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_transaction_by_block_number_and_index_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting transaction by block number and index with proper fields
    test_rpc_call_ok::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockNumberAndIndex",
        rpc_params!["0x29c", "0x0"],
    )
    .await;

    // Requesting transaction by block number and index with additional fields
    test_rpc_call_ok::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockNumberAndIndex",
        rpc_params!["0x29c", "0x0", true],
    )
    .await;

    // Requesting transaction by block number and index with missing fields
    test_rpc_call_err::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockNumberAndIndex",
        rpc_params![],
    )
    .await;

    // Requesting transaction by block number and index with wrong fields
    test_rpc_call_err::<Option<Transaction>>(
        &client,
        "eth_getTransactionByBlockNumberAndIndex",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_transaction_receipt_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting transaction receipt by transaction hash with proper fields
    test_rpc_call_ok::<Option<TransactionReceipt>>(
        &client,
        "eth_getTransactionReceipt",
        rpc_params!["0x85d995eba9763907fdf35cd2034144dd9d53ce32cbec21349d4b12823c6860c5"],
    )
    .await;

    // Requesting transaction receipt by transaction hash with additional fields
    test_rpc_call_ok::<Option<TransactionReceipt>>(
        &client,
        "eth_getTransactionReceipt",
        rpc_params!["0x85d995eba9763907fdf35cd2034144dd9d53ce32cbec21349d4b12823c6860c5", true],
    )
    .await;

    // Requesting transaction receipt by transaction hash with missing fields
    test_rpc_call_err::<Option<TransactionReceipt>>(
        &client,
        "eth_getTransactionReceipt",
        rpc_params![],
    )
    .await;

    // Requesting transaction receipt by transaction hash with wrong fields
    test_rpc_call_err::<Option<TransactionReceipt>>(
        &client,
        "eth_getTransactionReceipt",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_balance_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Vec of block number items
    let block_number = vec!["latest", "earliest", "pending", "0x2"];

    // Iterate over test cases
    for param in block_number {
        // Requesting balance by address with proper fields
        test_rpc_call_ok::<U256>(
            &client,
            "eth_getBalance",
            rpc_params!["0x407d73d8a49eeb85d32cf465507dd71d507100c1", param],
        )
        .await;
    }

    // Requesting balance by address with additional fields
    test_rpc_call_ok::<U256>(
        &client,
        "eth_getBalance",
        rpc_params!["0x407d73d8a49eeb85d32cf465507dd71d507100c1", "latest", true],
    )
    .await;

    // Requesting balance by address without block number which is optional
    test_rpc_call_ok::<U256>(
        &client,
        "eth_getBalance",
        rpc_params!["0x407d73d8a49eeb85d32cf465507dd71d507100c1"],
    )
    .await;

    // Requesting balance by address with no field
    test_rpc_call_err::<U256>(&client, "eth_getBalance", rpc_params![]).await;

    // Requesting balance by address with wrong fields
    test_rpc_call_err::<U256>(
        &client,
        "eth_getBalance",
        rpc_params![true], // Incorrect parameters
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_storage_at_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Vec of block number items
    let block_number = vec!["latest", "earliest", "pending", "0x2"];

    // Iterate over test cases
    for param in block_number {
        // Requesting storage at a given address with proper fields
        test_rpc_call_ok::<Bytes>(
            &client,
            "eth_getStorageAt",
            rpc_params![
                "0x295a70b2de5e3953354a6a8344e616ed314d7251", // Address
                "0x0",                                        // Position in the storage
                param                                         // Block number or tag
            ],
        )
        .await;
    }

    // Requesting storage at a given address with additional fields
    test_rpc_call_ok::<Bytes>(
        &client,
        "eth_getStorageAt",
        rpc_params![
            "0x295a70b2de5e3953354a6a8344e616ed314d7251", // Address
            "0x0",                                        // Position in the storage
            "latest",                                     // Block number or tag
            true                                          // Additional field
        ],
    )
    .await;

    // Requesting storage at a given address without block number which is optional
    test_rpc_call_ok::<Bytes>(
        &client,
        "eth_getStorageAt",
        rpc_params![
            "0x295a70b2de5e3953354a6a8344e616ed314d7251", // Address
            "0x0"                                         // Position in the storage
        ],
    )
    .await;

    // Requesting storage at a given address with no field
    test_rpc_call_err::<Bytes>(&client, "eth_getStorageAt", rpc_params![]).await;

    // Requesting storage at a given address with wrong fields
    test_rpc_call_err::<Bytes>(
        &client,
        "eth_getStorageAt",
        rpc_params![
            "0x295a70b2de5e3953354a6a8344e616ed314d7251", // Address
            "0x0",                                        // Position in the storage
            "not_valid_block_number"                      // Block number or tag
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_transaction_count_rpc_call() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Vec of block number items
    let block_number = vec!["latest", "earliest", "pending", "0x2"];

    // Iterate over test cases
    for param in block_number {
        // Requesting transaction count by address with proper fields
        test_rpc_call_ok::<U256>(
            &client,
            "eth_getTransactionCount",
            rpc_params![
                "0x407d73d8a49eeb85d32cf465507dd71d507100c1", // Address
                param                                         // Block number or tag
            ],
        )
        .await;
    }

    // Requesting transaction count by address with additional fields
    test_rpc_call_ok::<U256>(
        &client,
        "eth_getTransactionCount",
        rpc_params![
            "0x407d73d8a49eeb85d32cf465507dd71d507100c1", // Address
            "latest",                                     // Block number or tag
            true                                          // Additional field
        ],
    )
    .await;

    // Requesting transaction count by address without block number which is optional
    test_rpc_call_ok::<U256>(
        &client,
        "eth_getTransactionCount",
        rpc_params![
            "0x407d73d8a49eeb85d32cf465507dd71d507100c1" // Address
        ],
    )
    .await;

    // Requesting transaction count by address with no field
    test_rpc_call_err::<U256>(&client, "eth_getTransactionCount", rpc_params![]).await;

    // Requesting transaction count by address with wrong fields
    test_rpc_call_err::<U256>(
        &client,
        "eth_getTransactionCount",
        rpc_params!["0x407d73d8a49eeb85d32cf465507dd71d507100c1", "not_valid_block_number"], // Incorrect parameters
    )
    .await;
}

#[test]
fn test_rpc_builder_basic() {
    let rpc_string = RawRpcParamsBuilder::default()
        .method("eth_getBalance")
        .add_param("0xaa00000000000000000000000000000000000000")
        .add_param("0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2")
        .set_id(1)
        .build();

    let expected = r#"{"jsonrpc":"2.0","id":1,"method":"eth_getBalance","params":["0xaa00000000000000000000000000000000000000","0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2"]}"#;
    assert_eq!(rpc_string, expected, "RPC string did not match expected format.");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_fee_history_raw() {
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting block by number with proper fields
    test_rpc_call_ok::<Option<FeeHistory>>(
        &client,
        "eth_feeHistory",
        rpc_params!["0x0", "latest", [0]],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_debug_db_get() {
    reth_tracing::init_test_tracing();

    let handle = launch_http(vec![RethRpcModule::Debug]).await;
    let client = handle.http_client().unwrap();

    let valid_test_cases = [
        "0x630000000000000000000000000000000000000000000000000000000000000000",
        "c00000000000000000000000000000000",
    ];

    for key in valid_test_cases {
        client.request::<_, Option<Bytes>>("debug_dbGet", (key,)).await.unwrap();
    }

    // Invalid test cases
    let test_cases = [
        ("0x0000", "Key must be 33 bytes, got 2"),
        ("00", "Key must be 33 bytes, got 2"),
        (
            "0x000000000000000000000000000000000000000000000000000000000000000000",
            "Key prefix must be 0x63",
        ),
        ("000000000000000000000000000000000", "Key prefix must be 0x63"),
        ("0xc0000000000000000000000000000000000000000000000000000000000000000", "Invalid hex key"),
    ];

    let match_error_msg = |err: TransportError, expected: &str| -> bool {
        err.as_error_resp().is_some_and(|payload| {
            payload.code == i64::from(ErrorCode::InvalidParams.code()) &&
                payload.message == expected
        })
    };

    for (key, expected) in test_cases {
        let err = client.request::<_, Option<Bytes>>("debug_dbGet", (key,)).await.unwrap_err();
        assert!(match_error_msg(err, expected));
    }
}
