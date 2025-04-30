use crate::utils::{test_address, test_rpc_builder};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction};
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, RpcServiceBuilder},
    types::Request,
    MethodResponse,
};
use reth_rpc_builder::{RpcServerConfig, TransportRpcModuleConfig};
use reth_rpc_eth_api::EthApiClient;
use reth_rpc_server_types::RpcModuleSelection;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
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
    type MethodResponse = MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call(&self, req: Request<'a>) -> Self::Future {
        tracing::info!("MyMiddleware processed call {}", req.method);
        let count = self.count.clone();
        let service = self.service.clone();
        Box::pin(async move {
            let rp = service.call(req).await;
            // Modify the state.
            count.fetch_add(1, Ordering::Relaxed);
            rp
        })
    }

    fn batch(&self, req: Batch<'a>) -> Self::BatchResponse {
        self.service.batch(req)
    }

    fn notification(&self, n: Notification<'a>) -> Self::NotificationResponse {
        self.service.notification(n)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rpc_middleware() {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let modules =
        builder.build(TransportRpcModuleConfig::set_http(RpcModuleSelection::All), eth_api);

    let mylayer = MyMiddlewareLayer::default();

    let handle = RpcServerConfig::http(Default::default())
        .with_http_address(test_address())
        .set_rpc_middleware(RpcServiceBuilder::new().layer(mylayer.clone()))
        .start(&modules)
        .await
        .unwrap();

    let client = handle.http_client().unwrap();
    EthApiClient::<Transaction, Block, Receipt, Header>::protocol_version(&client).await.unwrap();
    let count = mylayer.count.load(Ordering::Relaxed);
    assert_eq!(count, 1);
}
