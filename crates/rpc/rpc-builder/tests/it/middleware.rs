use crate::utils::{test_address, test_rpc_builder};
use alloy_primitives::U64;
use alloy_transport::TransportError;
use jsonrpsee::{
    core::middleware::{Batch, Notification},
    server::middleware::rpc::RpcServiceT,
    types::Request,
    MethodResponse,
};
use reth_rpc_builder::{RpcServerConfig, TransportRpcModuleConfig};
use reth_rpc_server_types::RpcModuleSelection;
use reth_tokio_util::EventSender;
use serde_json::json;
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

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = MethodResponse> + Send + 'a {
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

#[tokio::test(flavor = "multi_thread")]
async fn test_rpc_params_limit() {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let modules = builder.build(
        TransportRpcModuleConfig::set_http(RpcModuleSelection::All),
        eth_api,
        EventSender::new(1),
    );

    let handle = RpcServerConfig::http(Default::default())
        .with_http_address(test_address())
        .with_max_params_containers(Some(4))
        .start(&modules)
        .await
        .unwrap();
    let client = handle.http_client().unwrap();
    let limit_err = "params exceed 4 JSON objects and arrays";
    let is_limit_err = |err: &TransportError| {
        err.as_error_resp()
            .is_some_and(|payload| payload.code == -32602 && payload.message == limit_err)
    };

    // The params array, the bundles array and two bundles are within the limit, so the method
    // decodes them.
    let err = client
        .request::<_, serde_json::Value>("eth_callMany", (json!([{}, {}]),))
        .await
        .unwrap_err();
    assert!(err.as_error_resp().is_some() && !is_limit_err(&err), "{err:?}");

    let err = client
        .request::<_, serde_json::Value>("eth_callMany", (json!([{}, {}, {}]),))
        .await
        .unwrap_err();
    assert!(is_limit_err(&err), "{err:?}");

    let mut batch = client.new_batch();
    let version = batch.add_call::<_, U64>("eth_protocolVersion", &()).unwrap();
    let call_many =
        batch.add_call::<_, serde_json::Value>("eth_callMany", &(json!([{}, {}, {}]),)).unwrap();
    batch.send().await.unwrap();
    version.await.unwrap();
    assert!(is_limit_err(&call_many.await.unwrap_err()));
}
