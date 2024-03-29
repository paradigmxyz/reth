#![allow(unreachable_pub)]
//! Standalone http tests

use crate::utils::{launch_http, launch_http_ws, launch_ws};
use jsonrpsee::{
    core::{
        client::{ClientT, SubscriptionClientT},
        error::Error,
        params::ArrayParams,
    },
    http_client::HttpClient,
    rpc_params,
    types::error::ErrorCode,
};
use reth_primitives::{
    hex_literal::hex, Address, BlockId, BlockNumberOrTag, Bytes, NodeRecord, TxHash, B256, B64,
    U256, U64,
};
use reth_rpc_api::{
    clients::{AdminApiClient, EthApiClient},
    DebugApiClient, EthFilterApiClient, NetApiClient, OtterscanClient, TraceApiClient,
    Web3ApiClient,
};
use reth_rpc_builder::RethRpcModule;
use reth_rpc_types::{
    trace::filter::TraceFilter, Filter, Index, Log, PendingTransactionFilterKind, RichBlock,
    SyncStatus, Transaction, TransactionReceipt, TransactionRequest,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

fn is_unimplemented(err: Error) -> bool {
    match err {
        Error::Call(error_obj) => {
            error_obj.code() == ErrorCode::InternalError.code() &&
                error_obj.message() == "unimplemented"
        }
        _ => false,
    }
}

async fn test_rpc_call_ok<R>(client: &HttpClient, method_name: &str, params: ArrayParams)
where
    R: DeserializeOwned,
{
    // Make the RPC request
    match client.request::<R, _>(method_name, params).await {
        Ok(_) => {} // If the request is successful, do nothing
        Err(e) => {
            // If an error occurs, panic with the error message
            panic!("Expected successful response, got error: {e:?}");
        }
    }
}

async fn test_rpc_call_err<R>(client: &HttpClient, method_name: &str, params: ArrayParams)
where
    R: DeserializeOwned + std::fmt::Debug,
{
    // Make the RPC request
    if let Ok(resp) = client.request::<R, _>(method_name, params).await {
        // Panic if an unexpected successful response is received
        panic!("Expected error response, got successful response: {resp:?}");
    };
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
    pub fn set_id(mut self, id: i32) -> Self {
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

async fn test_filter_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    EthFilterApiClient::new_filter(client, Filter::default()).await.unwrap();
    EthFilterApiClient::new_pending_transaction_filter(client, None).await.unwrap();
    EthFilterApiClient::new_pending_transaction_filter(
        client,
        Some(PendingTransactionFilterKind::Full),
    )
    .await
    .unwrap();
    let id = EthFilterApiClient::new_block_filter(client).await.unwrap();
    EthFilterApiClient::filter_changes(client, id.clone()).await.unwrap();
    EthFilterApiClient::logs(client, Filter::default()).await.unwrap();
    let id = EthFilterApiClient::new_filter(client, Filter::default()).await.unwrap();
    EthFilterApiClient::filter_logs(client, id.clone()).await.unwrap();
    EthFilterApiClient::uninstall_filter(client, id).await.unwrap();
}

async fn test_basic_admin_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
    let node: NodeRecord = url.parse().unwrap();

    AdminApiClient::add_peer(client, node).await.unwrap();
    AdminApiClient::remove_peer(client, node.into()).await.unwrap();
    AdminApiClient::add_trusted_peer(client, node.into()).await.unwrap();
    AdminApiClient::remove_trusted_peer(client, node.into()).await.unwrap();
    AdminApiClient::node_info(client).await.unwrap();
}

async fn test_basic_eth_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    let address = Address::default();
    let index = Index::default();
    let hash = B256::default();
    let tx_hash = TxHash::default();
    let block_number = BlockNumberOrTag::default();
    let call_request = TransactionRequest::default();
    let transaction_request = TransactionRequest::default();
    let bytes = Bytes::default();
    let tx = Bytes::from(hex!("02f871018303579880850555633d1b82520894eee27662c2b8eba3cd936a23f039f3189633e4c887ad591c62bdaeb180c080a07ea72c68abfb8fca1bd964f0f99132ed9280261bdca3e549546c0205e800f7d0a05b4ef3039e9c9b9babc179a1878fb825b5aaf5aed2fa8744854150157b08d6f3"));

    // Implemented
    EthApiClient::protocol_version(client).await.unwrap();
    EthApiClient::chain_id(client).await.unwrap();
    EthApiClient::accounts(client).await.unwrap();
    EthApiClient::block_number(client).await.unwrap();
    EthApiClient::get_code(client, address, None).await.unwrap();
    EthApiClient::send_raw_transaction(client, tx).await.unwrap();
    EthApiClient::fee_history(client, 0.into(), block_number, None).await.unwrap();
    EthApiClient::balance(client, address, None).await.unwrap();
    EthApiClient::transaction_count(client, address, None).await.unwrap();
    EthApiClient::storage_at(client, address, U256::default().into(), None).await.unwrap();
    EthApiClient::block_by_hash(client, hash, false).await.unwrap();
    EthApiClient::block_by_number(client, block_number, false).await.unwrap();
    EthApiClient::block_transaction_count_by_number(client, block_number).await.unwrap();
    EthApiClient::block_transaction_count_by_hash(client, hash).await.unwrap();
    EthApiClient::block_uncles_count_by_hash(client, hash).await.unwrap();
    EthApiClient::block_uncles_count_by_number(client, block_number).await.unwrap();
    EthApiClient::uncle_by_block_hash_and_index(client, hash, index).await.unwrap();
    EthApiClient::uncle_by_block_number_and_index(client, block_number, index).await.unwrap();
    EthApiClient::sign(client, address, bytes.clone()).await.unwrap_err();
    EthApiClient::sign_typed_data(client, address, jsonrpsee::core::JsonValue::Null)
        .await
        .unwrap_err();
    EthApiClient::transaction_by_hash(client, tx_hash).await.unwrap();
    EthApiClient::transaction_by_block_hash_and_index(client, hash, index).await.unwrap();
    EthApiClient::transaction_by_block_number_and_index(client, block_number, index).await.unwrap();
    EthApiClient::create_access_list(client, call_request.clone(), Some(block_number.into()))
        .await
        .unwrap();
    EthApiClient::estimate_gas(client, call_request.clone(), Some(block_number.into()), None)
        .await
        .unwrap();
    EthApiClient::call(client, call_request.clone(), Some(block_number.into()), None, None)
        .await
        .unwrap();
    EthApiClient::syncing(client).await.unwrap();
    EthApiClient::send_transaction(client, transaction_request).await.unwrap_err();
    EthApiClient::hashrate(client).await.unwrap();
    EthApiClient::submit_hashrate(client, U256::default(), B256::default()).await.unwrap();
    EthApiClient::gas_price(client).await.unwrap_err();
    EthApiClient::max_priority_fee_per_gas(client).await.unwrap_err();
    EthApiClient::get_proof(client, address, vec![], None).await.unwrap();

    // Unimplemented
    assert!(is_unimplemented(EthApiClient::author(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::is_mining(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::get_work(client).await.err().unwrap()));
    assert!(is_unimplemented(
        EthApiClient::submit_work(client, B64::default(), B256::default(), B256::default())
            .await
            .err()
            .unwrap()
    ));
    assert!(is_unimplemented(
        EthApiClient::sign_transaction(client, call_request.clone()).await.err().unwrap()
    ));
}

async fn test_basic_debug_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    let block_id = BlockId::Number(BlockNumberOrTag::default());

    DebugApiClient::raw_header(client, block_id).await.unwrap();
    DebugApiClient::raw_block(client, block_id).await.unwrap();
    DebugApiClient::raw_transaction(client, B256::default()).await.unwrap();
    DebugApiClient::raw_receipts(client, block_id).await.unwrap();
    assert!(is_unimplemented(DebugApiClient::bad_blocks(client).await.err().unwrap()));
}

async fn test_basic_net_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    NetApiClient::version(client).await.unwrap();
    NetApiClient::peer_count(client).await.unwrap();
    NetApiClient::is_listening(client).await.unwrap();
}

async fn test_basic_trace_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    let block_id = BlockId::Number(BlockNumberOrTag::default());
    let trace_filter = TraceFilter {
        from_block: Default::default(),
        to_block: Default::default(),
        from_address: Default::default(),
        to_address: Default::default(),
        mode: Default::default(),
        after: None,
        count: None,
    };

    TraceApiClient::trace_raw_transaction(client, Bytes::default(), HashSet::default(), None)
        .await
        .unwrap_err();
    TraceApiClient::trace_call_many(client, vec![], Some(BlockNumberOrTag::Latest.into()))
        .await
        .unwrap();
    TraceApiClient::replay_transaction(client, B256::default(), HashSet::default())
        .await
        .err()
        .unwrap();
    TraceApiClient::trace_block(client, block_id).await.unwrap();
    TraceApiClient::replay_block_transactions(client, block_id, HashSet::default()).await.unwrap();

    TraceApiClient::trace_filter(client, trace_filter).await.unwrap();
}

async fn test_basic_web3_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    Web3ApiClient::client_version(client).await.unwrap();
    Web3ApiClient::sha3(client, Bytes::default()).await.unwrap();
}

async fn test_basic_otterscan_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    let address = Address::default();
    let sender = Address::default();
    let tx_hash = TxHash::default();
    let block_number = BlockNumberOrTag::default();
    let page_number = 1;
    let page_size = 10;
    let nonce = 1;
    let block_hash = B256::default();

    OtterscanClient::has_code(client, address, None).await.unwrap();

    OtterscanClient::get_api_level(client).await.unwrap();

    OtterscanClient::get_internal_operations(client, tx_hash).await.unwrap();

    OtterscanClient::get_transaction_error(client, tx_hash).await.unwrap();

    assert!(is_unimplemented(
        OtterscanClient::trace_transaction(client, tx_hash).await.err().unwrap()
    ));

    OtterscanClient::get_block_details(client, block_number).await.unwrap();

    OtterscanClient::get_block_details_by_hash(client, block_hash).await.unwrap();

    OtterscanClient::get_block_transactions(client, block_number, page_number, page_size)
        .await
        .err()
        .unwrap();

    assert!(is_unimplemented(
        OtterscanClient::search_transactions_before(client, address, block_number, page_size,)
            .await
            .err()
            .unwrap()
    ));
    assert!(is_unimplemented(
        OtterscanClient::search_transactions_after(client, address, block_number, page_size,)
            .await
            .err()
            .unwrap()
    ));
    assert!(is_unimplemented(
        OtterscanClient::get_transaction_by_sender_and_nonce(client, sender, nonce,)
            .await
            .err()
            .unwrap()
    ));
    assert!(is_unimplemented(
        OtterscanClient::get_contract_creator(client, address).await.err().unwrap()
    ));
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

    let mut params = ArrayParams::default();
    params.insert( serde_json::json!({"blockHash":"0x58dc57ab582b282c143424bd01e8d923cddfdcda9455bad02a29522f6274a948"})).unwrap();

    let _resp = client.request::<Vec<Log>, _>("eth_getLogs", params).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_block_by_number_rpc_call() {
    // Initialize test tracing for logging
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting block by number with proper fields
    test_rpc_call_ok::<Option<RichBlock>>(
        &client,
        "eth_getBlockByNumber",
        rpc_params!["0x1b4", true], // Block number and full transaction object flag
    )
    .await;

    // Requesting block by number with wrong fields
    test_rpc_call_err::<Option<RichBlock>>(
        &client,
        "eth_getBlockByNumber",
        rpc_params!["0x1b4", "0x1b4"],
    )
    .await;

    // Requesting block by number with missing fields
    test_rpc_call_err::<Option<RichBlock>>(&client, "eth_getBlockByNumber", rpc_params!["0x1b4"])
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_block_by_hash_rpc_call() {
    // Initialize test tracing for logging
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting block by hash with proper fields
    test_rpc_call_ok::<Option<RichBlock>>(
        &client,
        "eth_getBlockByHash",
        rpc_params!["0xdc0818cf78f21a8e70579cb46a43643f78291264dda342ae31049421c82d21ae", false],
    )
    .await;

    // Requesting block by hash with wrong fields
    test_rpc_call_err::<Option<RichBlock>>(
        &client,
        "eth_getBlockByHash",
        rpc_params!["0xdc0818cf78f21a8e70579cb46a43643f78291264dda342ae31049421c82d21ae", "0x1b4"],
    )
    .await;

    // Requesting block by hash with missing fields
    test_rpc_call_err::<Option<RichBlock>>(
        &client,
        "eth_getBlockByHash",
        rpc_params!["0xdc0818cf78f21a8e70579cb46a43643f78291264dda342ae31049421c82d21ae"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_code_rpc_call() {
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting coinbase address without any parameter should return Unimplemented
    match client.request::<Address, _>("eth_coinbase", rpc_params![]).await {
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting uncle by block hash and index with proper fields
    test_rpc_call_ok::<Option<RichBlock>>(
        &client,
        "eth_getUncleByBlockHashAndIndex",
        rpc_params!["0xc6ef2fc5426d6ad6fd9e2a26abeab0aa2411b7ab17f30a99d3cb96aed1d1055b", "0x0"],
    )
    .await;

    // Requesting uncle by block hash and index with additional fields
    test_rpc_call_ok::<Option<RichBlock>>(
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
    test_rpc_call_err::<Option<RichBlock>>(
        &client,
        "eth_getUncleByBlockHashAndIndex",
        rpc_params![],
    )
    .await;

    // Requesting uncle by block hash and index with wrong fields
    test_rpc_call_err::<Option<RichBlock>>(
        &client,
        "eth_getUncleByBlockHashAndIndex",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_uncle_by_block_number_and_index_rpc_call() {
    // Initialize test tracing for logging
    reth_tracing::init_test_tracing();

    // Launch HTTP server with the specified RPC module
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    // Requesting uncle by block number and index with proper fields
    test_rpc_call_ok::<Option<RichBlock>>(
        &client,
        "eth_getUncleByBlockNumberAndIndex",
        rpc_params!["0x29c", "0x0"],
    )
    .await;

    // Requesting uncle by block number and index with additional fields
    test_rpc_call_ok::<Option<RichBlock>>(
        &client,
        "eth_getUncleByBlockNumberAndIndex",
        rpc_params!["0x29c", "0x0", true],
    )
    .await;

    // Requesting uncle by block number and index with missing fields
    test_rpc_call_err::<Option<RichBlock>>(
        &client,
        "eth_getUncleByBlockNumberAndIndex",
        rpc_params![],
    )
    .await;

    // Requesting uncle by block number and index with wrong fields
    test_rpc_call_err::<Option<RichBlock>>(
        &client,
        "eth_getUncleByBlockNumberAndIndex",
        rpc_params![true],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_transaction_by_hash_rpc_call() {
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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
    // Initialize test tracing for logging
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
