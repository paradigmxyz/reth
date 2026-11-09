use crate::utils::{test_address, test_rpc_builder};
use alloy_primitives::U64;
use alloy_rpc_server::{Batch, MethodResponse, Request, RpcServiceT};
use reth_rpc_builder::{RpcServerConfig, TransportRpcModuleConfig};
use reth_rpc_server_types::RpcModuleSelection;
use reth_tokio_util::EventSender;
use std::{
    future::Future,
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
    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
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

    fn batch<'a>(&self, req: Batch<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        self.service.batch(req)
    }

    fn notification<'a>(&self, n: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
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
    client.request_noparams::<U64>("eth_protocolVersion").await.unwrap();
    let count = mylayer.count.load(Ordering::Relaxed);
    assert_eq!(count, 1);
}
