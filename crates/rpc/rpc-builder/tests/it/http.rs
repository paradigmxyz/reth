//! Standalone http tests
use crate::utils::{launch_http, launch_http_ws, launch_ws};
use jsonrpsee::{
    core::{
        client::{ClientT, SubscriptionClientT},
        error::Error,
    },
    types::error::ErrorCode,
};
use reth_primitives::{
    hex_literal::hex, Address, BlockId, BlockNumberOrTag, Bytes, NodeRecord, TxHash, B256, B64,
    U256,
};
use reth_rpc_api::{
    clients::{AdminApiClient, EthApiClient},
    DebugApiClient, EthFilterApiClient, NetApiClient, OtterscanClient, TraceApiClient,
    Web3ApiClient,
};
use reth_rpc_builder::RethRpcModule;
use reth_rpc_types::{
    trace::filter::TraceFilter, CallRequest, Filter, Index, PendingTransactionFilterKind,
    TransactionRequest,
};
use serde::{Deserialize, Serialize};
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

/// Represents a builder for creating JSON-RPC requests.
#[derive(Serialize, Deserialize)]
struct RawRpcBuilder {
    endpoint: String,
    method: Option<String>,
    params: Vec<Value>,
    id: Option<i32>,
}

impl RawRpcBuilder {
    /// Creates a new `RawRpcBuilder` with a given endpoint.
    fn new(endpoint: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into(), method: None, params: Vec::new(), id: None }
    }

    /// Sets the method name for the JSON-RPC request.
    fn method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    /// Adds a parameter to the JSON-RPC request.
    fn add_param<S: Serialize>(mut self, param: S) -> Self {
        self.params.push(serde_json::to_value(param).expect("Failed to serialize parameter"));
        self
    }

    /// Sets the ID for the JSON-RPC request.
    fn set_id(mut self, id: i32) -> Self {
        self.id = Some(id);
        self
    }

    /// Constructs the JSON-RPC request string based on the provided configurations.
    fn build(self) -> String {
        let method = self.method.unwrap_or_else(|| panic!("JSON-RPC method not set"));
        let id = self.id.unwrap_or_else(|| panic!("JSON-RPC id not set"));
        let params: Vec<String> = self.params.into_iter().map(|p| p.to_string()).collect();

        format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"{}","params":[{}]}}"#,
            id,
            method,
            params.join(",")
        )
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
    AdminApiClient::remove_peer(client, node).await.unwrap();
    AdminApiClient::add_trusted_peer(client, node).await.unwrap();
    AdminApiClient::remove_trusted_peer(client, node).await.unwrap();
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
    let call_request = CallRequest::default();
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

    assert!(is_unimplemented(
        OtterscanClient::get_internal_operations(client, tx_hash).await.err().unwrap()
    ));
    assert!(is_unimplemented(
        OtterscanClient::get_transaction_error(client, tx_hash).await.err().unwrap()
    ));
    assert!(is_unimplemented(
        OtterscanClient::trace_transaction(client, tx_hash).await.err().unwrap()
    ));

    OtterscanClient::get_block_details(client, block_number).await.unwrap();

    OtterscanClient::get_block_details_by_hash(client, block_hash).await.unwrap();

    assert!(is_unimplemented(
        OtterscanClient::get_block_transactions(client, block_number, page_number, page_size,)
            .await
            .err()
            .unwrap()
    ));
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_builder_basic() {
        let rpc_string = RawRpcBuilder::new("http://localhost:8545")
            .method("eth_getBalance")
            .add_param("0xaa00000000000000000000000000000000000000")
            .add_param("0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2")
            .set_id(1)
            .build();

        let expected = r#"{"jsonrpc":"2.0","id":1,"method":"eth_getBalance","params":["0xaa00000000000000000000000000000000000000","0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2"]}"#;
        assert_eq!(rpc_string, expected, "RPC string did not match expected format.");
    }
}
