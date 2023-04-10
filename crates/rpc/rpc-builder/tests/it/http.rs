//! Standalone http tests

use crate::utils::{launch_http, launch_http_ws, launch_ws};
use jsonrpsee::{
    core::{
        client::{ClientT, SubscriptionClientT},
        error::Error,
    },
    types::error::{CallError, ErrorCode},
};
use reth_primitives::{
    hex_literal::hex, Address, BlockId, BlockNumberOrTag, Bytes, NodeRecord, TxHash, H256, H64,
    U256,
};
use reth_rpc_api::{
    clients::{AdminApiClient, EthApiClient},
    DebugApiClient, NetApiClient, TraceApiClient, Web3ApiClient,
};
use reth_rpc_builder::RethRpcModule;
use reth_rpc_types::{trace::filter::TraceFilter, CallRequest, Index, TransactionRequest};
use std::collections::HashSet;

fn is_unimplemented(err: Error) -> bool {
    match err {
        Error::Call(CallError::Custom(error_obj)) => {
            error_obj.code() == ErrorCode::InternalError.code() &&
                error_obj.message() == "unimplemented"
        }
        _ => false,
    }
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
    let hash = H256::default();
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
    EthApiClient::fee_history(client, 0.into(), block_number.into(), None).await.unwrap();
    EthApiClient::balance(client, address, None).await.unwrap();
    EthApiClient::transaction_count(client, address, None).await.unwrap();
    EthApiClient::storage_at(client, address, U256::default().into(), None).await.unwrap();
    EthApiClient::block_by_hash(client, hash, false).await.unwrap();
    EthApiClient::block_by_number(client, block_number, false).await.unwrap();
    EthApiClient::block_transaction_count_by_number(client, block_number).await.unwrap();
    EthApiClient::block_transaction_count_by_hash(client, hash).await.unwrap();
    EthApiClient::get_proof(client, address, vec![], None).await.unwrap();
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
    EthApiClient::estimate_gas(client, call_request.clone(), Some(block_number.into()))
        .await
        .unwrap();
    EthApiClient::call(client, call_request.clone(), Some(block_number.into()), None)
        .await
        .unwrap();
    EthApiClient::syncing(client).await.unwrap();
    EthApiClient::send_transaction(client, transaction_request).await.unwrap_err();
    EthApiClient::hashrate(client).await.unwrap();
    EthApiClient::submit_hashrate(client, U256::default(), H256::default()).await.unwrap();

    // Unimplemented
    assert!(is_unimplemented(EthApiClient::author(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::gas_price(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::max_priority_fee_per_gas(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::is_mining(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::get_work(client).await.err().unwrap()));
    assert!(is_unimplemented(
        EthApiClient::submit_work(client, H64::default(), H256::default(), H256::default())
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
    DebugApiClient::raw_transaction(client, H256::default()).await.unwrap();
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
        from_block: None,
        to_block: None,
        from_address: None,
        to_address: None,
        after: None,
        count: None,
    };

    TraceApiClient::trace_raw_transaction(client, Bytes::default(), HashSet::default(), None)
        .await
        .unwrap_err();
    TraceApiClient::trace_call_many(client, vec![], None).await.err().unwrap();
    TraceApiClient::replay_transaction(client, H256::default(), HashSet::default())
        .await
        .err()
        .unwrap();
    TraceApiClient::trace_block(client, block_id).await.err().unwrap();

    assert!(is_unimplemented(
        TraceApiClient::replay_block_transactions(client, block_id, HashSet::default())
            .await
            .err()
            .unwrap()
    ));
    assert!(is_unimplemented(
        TraceApiClient::trace_filter(client, trace_filter).await.err().unwrap()
    ));
}

async fn test_basic_web3_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    Web3ApiClient::client_version(client).await.unwrap();
    Web3ApiClient::sha3(client, Bytes::default()).await.unwrap();
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
