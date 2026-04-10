//! Example of how to create a node with custom middleware that alters a returned error message from
//! the RPC
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-custom-rpc-middleware node --http --dev --dev.block-time 12s --http.api=debug,eth
//! ```
//!
//! Then make an RPC request that will result in an error
//!
//! ```sh
//! curl -s -X POST http://localhost:8545 \
//!  -H "Content-Type: application/json" \
//!  -d '{
//!    "jsonrpc": "2.0",
//!    "method": "debug_getRawBlock",
//!    "params": ["2"],
//!   "id": 1
//! }' | jq
//! ```

use clap::Parser;
use jsonrpsee::{
    core::{
        middleware::{Batch, Notification, RpcServiceT},
        server::MethodResponse,
    },
    types::{ErrorObjectOwned, Id, Request},
};
use reth_ethereum::{
    cli::{chainspec::EthereumChainSpecParser, interface::Cli},
    node::{EthereumAddOns, EthereumNode},
};
use tower::Layer;

fn main() {
    Cli::<EthereumChainSpecParser>::parse()
        .run(async move |builder, _| {
            let handle = builder
                .with_types::<EthereumNode>()
                .with_components(EthereumNode::components())
                .with_add_ons(
                    //create ethereum addons with our custom rpc middleware
                    EthereumAddOns::default().with_rpc_middleware(ResponseMutationLayer),
                )
                .launch_with_debug_capabilities()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

#[derive(Clone)]
pub struct ResponseMutationLayer;

impl<S> Layer<S> for ResponseMutationLayer {
    type Service = ResponseMutationService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ResponseMutationService { service: inner }
    }
}

#[derive(Clone)]
pub struct ResponseMutationService<S> {
    service: S,
}

impl<S> RpcServiceT for ResponseMutationService<S>
where
    S: RpcServiceT<
            MethodResponse = jsonrpsee::MethodResponse,
            BatchResponse = jsonrpsee::MethodResponse,
            NotificationResponse = jsonrpsee::MethodResponse,
        > + Send
        + Sync
        + Clone
        + 'static,
{
    type MethodResponse = S::MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        tracing::info!("processed call {:?}", req);
        let service = self.service.clone();
        async move {
            let resp = service.call(req).await;

            //we can modify the response with our own custom error
            if resp.is_error() {
                let err = ErrorObjectOwned::owned(
                    -31404,
                    "CustomError",
                    Some("Our very own custom error message"),
                );
                return MethodResponse::error(Id::Number(1), err);
            }

            //otherwise just return the original response
            resp
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
