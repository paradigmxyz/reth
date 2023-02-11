//! Standalone http tests

use crate::utils::{launch_http, launch_http_ws, launch_ws};
use jsonrpsee::core::client::{ClientT, SubscriptionClientT};
use reth_primitives::{NodeRecord, H256, BlockNumber, U256, H64, rpc::{BlockId, BlockNumber as RpcBlockNumber }};
use reth_rpc_api::DebugApiClient;
use reth_rpc_api::clients::{AdminApiClient, EthApiClient};
use reth_rpc_builder::RethRpcModule;
use jsonrpsee::core::error::Error;
use jsonrpsee::types::error::{CallError, ErrorCode};

fn is_unimplemented(err: Error) -> bool {
    match err {
        Error::Call(CallError::Custom(error_obj)) => {
            error_obj.code() == ErrorCode::InternalError.code() && error_obj.message() == "unimplemented"
        }
        _ => return false,
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
where C: ClientT + SubscriptionClientT + Sync {

    // Implemented
    EthApiClient::protocol_version(client).await.unwrap();
    EthApiClient::chain_id(client).await.unwrap();
    EthApiClient::chain_id(client).await.unwrap();
    EthApiClient::accounts(client).await.unwrap();
    EthApiClient::block_number(client).await.unwrap();


    // Unimplemented
    assert!(is_unimplemented(EthApiClient::syncing(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::author(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::block_by_hash(client, H256::default(), false).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::block_by_number(client, BlockNumber::default(), false).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::block_transaction_count_by_hash(client, H256::default()).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::block_transaction_count_by_number(client, BlockNumber::default()).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::block_uncles_count_by_hash(client, H256::default()).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::block_uncles_count_by_number(client, BlockNumber::default()).await.err().unwrap()));
    // EthApiClient::uncle_by_block_hash_and_index(client, H256::default(), Index::default()).await.err().unwrap()));
    // EthApiClient::uncle_by_block_number_and_index(client).await.err().unwrap()));
    // EthApiClient::transaction_by_hash(client, H256::default()).await.err().unwrap()));
    // EthApiClient::transaction_by_block_hash_and_index(client).await.err().unwrap()));
    // EthApiClient::transaction_by_block_number_and_index(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::transaction_receipt(client, H256::default()).await.err().unwrap()));
    // EthApiClient::balance(client).await.err().unwrap()));
    // EthApiClient::storage_at(client).await.err().unwrap()));
    // EthApiClient::transaction_count(client).await.err().unwrap()));
    // EthApiClient::get_code(client).await.err().unwrap()));
    // EthApiClient::call(client).await.err().unwrap()));
    // EthApiClient::create_access_list(client).await.err().unwrap()));
    // EthApiClient::estimate_gas(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::gas_price(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::fee_history(client, U256::default(), BlockNumber::default(), None).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::max_priority_fee_per_gas(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::is_mining(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::hashrate(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::get_work(client).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::submit_hashrate(client, U256::default(), H256::default()).await.err().unwrap()));
    assert!(is_unimplemented(EthApiClient::submit_work(client, H64::default(), H256::default(), H256::default()).await.err().unwrap()));
    // EthApiClient::send_transaction(client).await.err().unwrap()));
    // EthApiClient::send_raw_transaction(client).await.err().unwrap()));
    // EthApiClient::sign(client).await.err().unwrap()));
    // EthApiClient::sign_transaction(client).await.err().unwrap()));
    // EthApiClient::sign_typed_data(client).await.err().unwrap()));
    // EthApiClient::get_proof(client).await.err().unwrap()));

}
    
async fn test_basic_debug_calls<C>(client: &C)
where C: ClientT + SubscriptionClientT + Sync {
    let block_id = BlockId::Number(RpcBlockNumber::default());

    assert!(is_unimplemented(DebugApiClient::raw_block(client, block_id).await.err().unwrap()));
    assert!(is_unimplemented(DebugApiClient::raw_transaction(client, H256::default()).await.err().unwrap()));
    assert!(is_unimplemented(DebugApiClient::raw_header(client, block_id).await.err().unwrap()));
    assert!(is_unimplemented(DebugApiClient::raw_receipts(client, block_id).await.err().unwrap()));
    assert!(is_unimplemented(DebugApiClient::bad_blocks(client).await.err().unwrap()));
}

async fn test_basic_net_calls<C>(client: &C)
where C: ClientT + SubscriptionClientT + Sync {

}

async fn test_basic_trace_calls<C>(client: &C)
where C: ClientT + SubscriptionClientT + Sync {

}

async fn test_basic_web3_calls<C>(client: &C)
where C: ClientT + SubscriptionClientT + Sync {

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
