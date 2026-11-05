use crate::utils::{test_address, test_rpc_builder};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction, TransactionRequest};
use jsonrpsee::{
    client_transport::ws::WsTransportClientBuilder,
    core::{
        client::{ReceivedMessage, TransportReceiverT, TransportSenderT},
        middleware::{Batch, Notification},
    },
    server::{middleware::rpc::RpcServiceT, ServerConfigBuilder},
    types::{error::TOO_BIG_BATCH_RESPONSE_CODE, Request},
    RpcModule,
};
use reth_ethereum_primitives::TransactionSigned;
use reth_rpc_builder::{RpcServerConfig, TransportRpcModuleConfig, TransportRpcModules};
use reth_rpc_eth_api::EthApiClient;
use reth_rpc_server_types::{BatchResponseCommit, RpcModuleSelection};
use reth_tokio_util::EventSender;
use serde_json::{json, Value};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::timeout;
use tower::Layer;

#[derive(Clone, Default)]
struct MyMiddlewareLayer {
    count: Arc<AtomicUsize>,
}

impl<S> Layer<S> for MyMiddlewareLayer {
    type Service = MyMiddlewareService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MyMiddlewareService { service: inner, count: self.count.clone() }
    }
}

#[derive(Clone)]
struct MyMiddlewareService<S> {
    service: S,
    count: Arc<AtomicUsize>,
}

impl<S> RpcServiceT for MyMiddlewareService<S>
where
    S: RpcServiceT + Send + Sync + Clone + 'static,
{
    type MethodResponse = S::MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        tracing::info!("MyMiddleware processed call {}", req.method);
        let count = self.count.clone();
        let service = self.service.clone();
        async move {
            let rp = service.call(req).await;
            // Modify the state.
            count.fetch_add(1, Ordering::Relaxed);
            rp
        }
    }

    fn batch<'a>(&self, req: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        self.service.batch(req)
    }

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        self.service.notification(n)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rpc_middleware() {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let modules = builder.build(
        TransportRpcModuleConfig::set_http(RpcModuleSelection::All),
        eth_api,
        EventSender::new(1),
    );

    let mylayer = MyMiddlewareLayer::default();

    let handle = RpcServerConfig::http(Default::default())
        .with_http_address(test_address())
        .set_rpc_middleware(mylayer.clone())
        .start(&modules)
        .await
        .unwrap();

    let client = handle.http_client().unwrap();
    EthApiClient::<TransactionRequest, Transaction, Block, Receipt, Header, TransactionSigned>::protocol_version(
        &client,
    )
    .await
    .unwrap();
    let count = mylayer.count.load(Ordering::Relaxed);
    assert_eq!(count, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_updates_require_complete_response() {
    let count = Arc::new(AtomicUsize::new(0));
    let mut module = RpcModule::new(count.clone());
    module
        .register_method("poll", |_, count, extensions| {
            let count = count.clone();
            extensions.get::<BatchResponseCommit>().unwrap().defer(move || {
                count.fetch_add(1, Ordering::Relaxed);
            });
            "x".repeat(64)
        })
        .unwrap();
    let module = module.remove_context();
    let modules = TransportRpcModules::default().with_http(module.clone()).with_ws(module);
    let config = ServerConfigBuilder::default().max_response_body_size(128);
    for server in [
        RpcServerConfig::http(config.clone()).with_http_address(test_address()),
        RpcServerConfig::ws(config.clone()).with_ws_address(test_address()),
        RpcServerConfig::http(config.clone())
            .with_ws(config)
            .with_http_address(test_address())
            .with_ws_address(test_address()),
    ] {
        let handle = server.start(&modules).await.unwrap();
        if let Some(url) = handle.http_url() {
            count.store(0, Ordering::Relaxed);
            for requests in [2, 1] {
                let response = reqwest::Client::new()
                    .post(&url)
                    .header("content-type", "application/json")
                    .body(batch_request(requests))
                    .timeout(Duration::from_secs(30))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                assert_batch_response(&response, &count, requests);
            }
        }
        if let Some(url) = handle.ws_url() {
            count.store(0, Ordering::Relaxed);
            let (mut sender, mut receiver) =
                WsTransportClientBuilder::default().build(url.parse().unwrap()).await.unwrap();
            for requests in [2, 1] {
                sender.send(batch_request(requests)).await.unwrap();
                let ReceivedMessage::Text(response) =
                    timeout(Duration::from_secs(30), receiver.receive()).await.unwrap().unwrap()
                else {
                    panic!("expected text response")
                };
                assert_batch_response(&response, &count, requests);
            }
        }
        handle.stop().unwrap();
    }
}

fn batch_request(requests: usize) -> String {
    serde_json::to_string(
        &(0..requests)
            .map(|id| json!({"jsonrpc": "2.0", "id": id, "method": "poll"}))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn assert_batch_response(response: &str, count: &AtomicUsize, requests: usize) {
    let response = serde_json::from_str::<Value>(response).unwrap();
    if requests == 2 {
        assert_eq!(response["error"]["code"], TOO_BIG_BATCH_RESPONSE_CODE);
        assert_eq!(count.load(Ordering::Relaxed), 0);
    } else {
        assert_eq!(response, json!([{"jsonrpc": "2.0", "id": 0, "result": "x".repeat(64)}]));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
