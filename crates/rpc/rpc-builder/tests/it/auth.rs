//! Auth server tests

use crate::utils::launch_auth;
use jsonrpsee::core::client::{ClientT, SubscriptionClientT};
use reth_primitives::{Block, U64};
use reth_rpc::JwtSecret;
use reth_rpc_api::clients::EngineApiClient;
use reth_rpc_types::engine::{ForkchoiceState, PayloadId, TransitionConfiguration};
use reth_rpc_types_compat::engine::payload::{
    convert_block_to_payload_input_v2, try_block_to_payload_v1,
};
#[allow(unused_must_use)]
async fn test_basic_engine_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    let block = Block::default().seal_slow();
    EngineApiClient::new_payload_v1(client, try_block_to_payload_v1(block.clone())).await;
    EngineApiClient::new_payload_v2(client, convert_block_to_payload_input_v2(block)).await;
    EngineApiClient::fork_choice_updated_v1(client, ForkchoiceState::default(), None).await;
    EngineApiClient::get_payload_v1(client, PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0])).await;
    EngineApiClient::get_payload_v2(client, PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0])).await;
    EngineApiClient::get_payload_bodies_by_hash_v1(client, vec![]).await;
    EngineApiClient::get_payload_bodies_by_range_v1(client, U64::ZERO, U64::from(1u64)).await;
    EngineApiClient::exchange_transition_configuration(client, TransitionConfiguration::default())
        .await;
    EngineApiClient::exchange_capabilities(client, vec![]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_auth_endpoints_http() {
    reth_tracing::init_test_tracing();
    let secret = JwtSecret::random();
    let handle = launch_auth(secret).await;
    let client = handle.http_client();
    test_basic_engine_calls(&client).await
}

#[tokio::test(flavor = "multi_thread")]
async fn test_auth_endpoints_ws() {
    reth_tracing::init_test_tracing();
    let secret = JwtSecret::random();
    let handle = launch_auth(secret).await;
    let client = handle.ws_client().await;
    test_basic_engine_calls(&client).await
}
