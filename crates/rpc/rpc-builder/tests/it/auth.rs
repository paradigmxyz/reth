//! Auth server tests

use crate::utils::launch_auth;
use jsonrpsee::core::client::{ClientT, SubscriptionClientT};
use reth_primitives::Block;
use reth_rpc::JwtSecret;
use reth_rpc_api::clients::EngineApiClient;
use reth_rpc_types::engine::{ForkchoiceState, PayloadId, TransitionConfiguration};

#[allow(unused_must_use)]
async fn test_basic_engine_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync,
{
    let block = Block::default().seal_slow();
    EngineApiClient::new_payload_v1(client, block.clone().into()).await;
    EngineApiClient::new_payload_v2(client, block.into()).await;
    EngineApiClient::fork_choice_updated_v1(client, ForkchoiceState::default(), None).await;
    EngineApiClient::get_payload_v1(client, PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0])).await;
    EngineApiClient::get_payload_v2(client, PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0])).await;
    EngineApiClient::get_payload_bodies_by_hash_v1(client, vec![]).await;
    EngineApiClient::get_payload_bodies_by_range_v1(client, 0u64.into(), 1u64.into()).await;
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
