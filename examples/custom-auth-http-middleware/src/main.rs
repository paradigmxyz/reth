//! Example of how to create a node with custom auth HTTP middleware that proxies requests based on
//! the URL path.
//!
//! This middleware operates at the HTTP transport layer on the auth/Engine API server (port 8551),
//! applied **after** JWT authentication and **before** JSON-RPC parsing. It inspects the raw HTTP
//! request path and can proxy matching requests to an upstream server, while forwarding everything
//! else to the default Engine API handler.
//!
//! ## Architecture
//!
//! ```text
//! HTTP Request → JWT Auth → PathProxyMiddleware → JSON-RPC Parsing → Engine API Handler
//!                                 │
//!                                 └── if path matches "/proxy/*" → forward to upstream
//! ```
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-custom-auth-http-middleware node \
//!     --authrpc.jwtsecret /tmp/jwt.hex \
//!     --dev --dev.block-time 12s
//! ```
//!
//! Then send an authenticated request to the proxy path:
//!
//! ```sh
//! # Generate a JWT token for the secret in /tmp/jwt.hex and send a proxied request:
//! curl -s -X POST http://localhost:8551/proxy/anything \
//!   -H "Content-Type: application/json" \
//!   -H "Authorization: Bearer <jwt-token>" \
//!   -d '{"hello":"world"}'
//! ```

use clap::Parser;
use http::{header::CONTENT_TYPE, Response, StatusCode};
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_ethereum::{
    cli::{chainspec::EthereumChainSpecParser, interface::Cli},
    node::{EthereumAddOns, EthereumNode},
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{BoxError, Layer, Service};

fn main() {
    Cli::<EthereumChainSpecParser>::parse()
        .run(async move |builder, _| {
            let handle = builder
                .with_types::<EthereumNode>()
                .with_components(EthereumNode::components())
                .with_add_ons(
                    EthereumAddOns::default()
                        .with_auth_http_middleware(PathProxyLayer { proxy_prefix: "/proxy/" }),
                )
                .launch_with_debug_capabilities()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// A [`Layer`] that intercepts HTTP requests matching a path prefix and returns a custom
/// response instead of forwarding to the Engine API.
///
/// In a real-world scenario, you would forward matching requests to an upstream HTTP server
/// (e.g., via `hyper` or `reqwest`). This example returns a simple JSON response to demonstrate
/// the path-based routing pattern.
#[derive(Clone)]
struct PathProxyLayer {
    proxy_prefix: &'static str,
}

impl<S> Layer<S> for PathProxyLayer {
    type Service = PathProxyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PathProxyService { inner, proxy_prefix: self.proxy_prefix }
    }
}

/// The [`Service`] produced by [`PathProxyLayer`].
///
/// For requests whose path starts with the configured prefix, this service short-circuits the
/// normal Engine API pipeline and returns a custom HTTP response. All other requests are forwarded
/// unchanged to the inner service (the JSON-RPC handler).
#[derive(Clone)]
struct PathProxyService<S> {
    inner: S,
    proxy_prefix: &'static str,
}

impl<S> Service<HttpRequest> for PathProxyService<S>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = BoxError> + Send + Clone,
    S::Future: Send + 'static,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        let path = req.uri().path().to_owned();
        let proxy_prefix = self.proxy_prefix;

        if let Some(suffix) = path.strip_prefix(proxy_prefix) {
            // The request path matches our proxy prefix — handle it here instead of forwarding
            // to the Engine API JSON-RPC handler.
            //
            // In production, you might forward this to an upstream HTTP server:
            //   let upstream_url = format!("http://127.0.0.1:9000{}", path);
            //   let resp = reqwest::Client::new().post(&upstream_url).body(body).send().await?;
            tracing::info!(path = %path, "intercepted request on proxy path");

            let body = serde_json::json!({
                "status": "proxied",
                "matched_prefix": proxy_prefix,
                "remaining_path": suffix,
            });

            let response = Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "application/json")
                .body(HttpBody::from(body.to_string()))
                .expect("valid response");

            Box::pin(async move { Ok(response) })
        } else {
            // Not a proxy path — forward to the inner Engine API handler as usual.
            let fut = self.inner.call(req);
            Box::pin(fut)
        }
    }
}
