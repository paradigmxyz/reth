//! Standalone http tests

use crate::utils::{launch_http, launch_http_ws, launch_ws};
use jsonrpsee::core::client::{ClientT, SubscriptionClientT};
use reth_primitives::{NodeRecord, H256, BlockNumber, U256, H64};
use reth_rpc_api::{clients::AdminApiClient, EthApiClient};
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
    EthApiClient::author(client).await.unwrap_err();
    EthApiClient::block_by_hash(client, H256::default(), false).await.unwrap_err();
    EthApiClient::block_by_number(client, BlockNumber::default(), false).await.unwrap_err();
    EthApiClient::block_transaction_count_by_hash(client, H256::default()).await.unwrap_err();
    EthApiClient::block_transaction_count_by_number(client, BlockNumber::default()).await.unwrap_err();
    EthApiClient::block_uncles_count_by_hash(client, H256::default()).await.unwrap_err();
    EthApiClient::block_uncles_count_by_number(client, BlockNumber::default()).await.unwrap_err();
    // EthApiClient::uncle_by_block_hash_and_index(client, H256::default(), Index::default()).await.unwrap_err();
    // EthApiClient::uncle_by_block_number_and_index(client).await.unwrap_err();
    // EthApiClient::transaction_by_hash(client, H256::default()).await.unwrap_err();
    // EthApiClient::transaction_by_block_hash_and_index(client).await.unwrap_err();
    // EthApiClient::transaction_by_block_number_and_index(client).await.unwrap_err();
    EthApiClient::transaction_receipt(client, H256::default()).await.unwrap_err();
    // EthApiClient::balance(client).await.unwrap_err();
    // EthApiClient::storage_at(client).await.unwrap_err();
    // EthApiClient::transaction_count(client).await.unwrap_err();
    // EthApiClient::get_code(client).await.unwrap_err();
    // EthApiClient::call(client).await.unwrap_err();
    // EthApiClient::create_access_list(client).await.unwrap_err();
    // EthApiClient::estimate_gas(client).await.unwrap_err();
    EthApiClient::gas_price(client).await.unwrap_err();
    EthApiClient::fee_history(client, U256::default(), BlockNumber::default(), None).await.unwrap_err();
    EthApiClient::max_priority_fee_per_gas(client).await.unwrap_err();
    EthApiClient::is_mining(client).await.unwrap_err();
    EthApiClient::hashrate(client).await.unwrap_err();
    EthApiClient::get_work(client).await.unwrap_err();
    EthApiClient::submit_hashrate(client, U256::default(), H256::default()).await.unwrap_err();
    EthApiClient::submit_work(client, H64::default(), H256::default(), H256::default()).await.unwrap_err();
    // EthApiClient::send_transaction(client).await.unwrap_err();
    // EthApiClient::send_raw_transaction(client).await.unwrap_err();
    // EthApiClient::sign(client).await.unwrap_err();
    // EthApiClient::sign_transaction(client).await.unwrap_err();
    // EthApiClient::sign_typed_data(client).await.unwrap_err();
    // EthApiClient::get_proof(client).await.unwrap_err();

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
