//! Auth server tests

use crate::utils::{launch_auth, launch_auth_with_config, test_address};
use alloy_primitives::U64;
use alloy_rpc_types_engine::{
    ExecutionPayloadInputV2, ExecutionPayloadV1, ForkchoiceState, PayloadId,
};
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use jsonrpsee::{
    core::client::{ClientT, SubscriptionClientT},
    server::{HttpRequest, HttpResponse},
};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_primitives::{Block, TransactionSigned};
use reth_primitives_traits::block::Block as _;
use reth_rpc_api::clients::EngineApiClient;
use reth_rpc_builder::auth::AuthServerConfig;
use reth_rpc_layer::{secret_to_bearer_header, JwtSecret};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};
use tower::{Layer, Service};

#[derive(Clone, Default)]
struct CountingAuthHttpLayer {
    count: Arc<AtomicUsize>,
    content_types: Arc<Mutex<Vec<Option<String>>>>,
}

impl CountingAuthHttpLayer {
    fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    fn seen_content_types(&self) -> Vec<Option<String>> {
        self.content_types.lock().unwrap().clone()
    }
}

impl<S> Layer<S> for CountingAuthHttpLayer {
    type Service = CountingAuthHttpService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CountingAuthHttpService {
            inner,
            count: self.count.clone(),
            content_types: self.content_types.clone(),
        }
    }
}

#[derive(Clone)]
struct CountingAuthHttpService<S> {
    inner: S,
    count: Arc<AtomicUsize>,
    content_types: Arc<Mutex<Vec<Option<String>>>>,
}

impl<S> Service<HttpRequest> for CountingAuthHttpService<S>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = tower::BoxError> + Send + Clone,
    S::Future: Send + 'static,
{
    type Response = HttpResponse;
    type Error = tower::BoxError;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: HttpRequest) -> Self::Future {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.content_types.lock().unwrap().push(
            request
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        );
        self.inner.call(request)
    }
}

#[expect(unused_must_use)]
async fn test_basic_engine_calls<C>(client: &C)
where
    C: ClientT + SubscriptionClientT + Sync + EngineApiClient<EthEngineTypes>,
{
    let block = Block::default().seal_slow();
    EngineApiClient::new_payload_v1(
        client,
        ExecutionPayloadV1::from_block_unchecked(block.hash(), &block.clone().into_block()),
    )
    .await;
    EngineApiClient::new_payload_v2(
        client,
        ExecutionPayloadInputV2 {
            execution_payload: ExecutionPayloadV1::from_block_slow::<TransactionSigned, _>(
                &block.into_block(),
            ),
            withdrawals: None,
        },
    )
    .await;
    EngineApiClient::fork_choice_updated_v1(client, ForkchoiceState::default(), None).await;
    EngineApiClient::get_payload_v1(client, PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0])).await;
    EngineApiClient::get_payload_v2(client, PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0])).await;
    EngineApiClient::get_payload_bodies_by_hash_v1(client, vec![]).await;
    EngineApiClient::get_payload_bodies_by_range_v1(client, U64::ZERO, U64::from(1u64)).await;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_auth_http_middleware_runs_only_after_jwt() {
    reth_tracing::init_test_tracing();

    let secret = JwtSecret::random();
    let layer = CountingAuthHttpLayer::default();
    let config = AuthServerConfig::builder(secret).socket_addr(test_address()).build();
    let handle = launch_auth_with_config(config.with_http_middleware(layer.clone())).await;

    let response = reqwest::Client::new()
        .post(handle.http_url())
        .header(CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "engine_exchangeCapabilities",
                "params": [[]],
                "id": 1
            }))
            .unwrap(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(layer.count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_auth_http_middleware_sees_transport_headers_before_json_rpc_parsing() {
    reth_tracing::init_test_tracing();

    let secret = JwtSecret::random();
    let layer = CountingAuthHttpLayer::default();
    let config = AuthServerConfig::builder(secret).socket_addr(test_address()).build();
    let handle = launch_auth_with_config(config.with_http_middleware(layer.clone())).await;

    let response = reqwest::Client::new()
        .post(handle.http_url())
        .header(AUTHORIZATION, secret_to_bearer_header(&secret))
        .header(CONTENT_TYPE, "application/ssz")
        .body("not-json")
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success() || response.status().is_client_error());
    assert_eq!(layer.count(), 1);
    assert_eq!(layer.seen_content_types(), vec![Some("application/ssz".to_owned())]);
}
