//! Middleware for the RPC service.

pub mod layer;

use std::{
    borrow::Cow,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::server::MethodResponse;
use futures_util::future::Either;
use jsonrpsee_types::{ErrorObject, Id};
use pin_project::pin_project;
use serde_json::value::RawValue;
use tower::layer::{
    util::{Identity, Stack},
    LayerFn,
};

/// Re-export types from `jsonrpsee_types` crate for convenience.
pub type Notification<'a> = jsonrpsee_types::Notification<'a, Option<Cow<'a, RawValue>>>;
/// Re-export types from `jsonrpsee_types` crate for convenience.
pub use jsonrpsee_types::Request;

/// Error response that can used to indicate an error in JSON-RPC request batch request.
/// This is used in the [`Batch`] type to indicate an error in the batch entry.
#[derive(Debug)]
pub struct BatchEntryErr<'a> {
    id: Id<'a>,
    err: ErrorObject<'a>,
}

impl<'a> BatchEntryErr<'a> {
    /// Create a new error response.
    pub const fn new(id: Id<'a>, err: ErrorObject<'a>) -> Self {
        Self { id, err }
    }

    /// Get the parts of the error response.
    pub fn into_parts(self) -> (ErrorObject<'a>, Id<'a>) {
        (self.err, self.id)
    }
}

/// A batch of JSON-RPC requests.
#[derive(Debug)]
pub struct Batch<'a> {
    inner: Vec<Result<BatchEntry<'a>, BatchEntryErr<'a>>>,
}

impl<'a> IntoIterator for Batch<'a> {
    type Item = Result<BatchEntry<'a>, BatchEntryErr<'a>>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl<'a> Batch<'a> {
    /// Create a new batch from a vector of batch entries.
    pub fn from(entries: Vec<Result<BatchEntry<'a>, BatchEntryErr<'a>>>) -> Self {
        Self { inner: entries }
    }

    /// Get the length of the batch.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns whether the batch is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Get an iterator over the batch.
    pub fn iter(&self) -> impl Iterator<Item = &Result<BatchEntry<'a>, BatchEntryErr<'a>>> {
        self.inner.iter()
    }

    /// Get a mutable iterator over the batch.
    pub fn iter_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut Result<BatchEntry<'a>, BatchEntryErr<'a>>> {
        self.inner.iter_mut()
    }
}

/// A batch entry specific for the [`RpcServiceT::batch`] method to support both
/// method calls and notifications.
#[derive(Debug, Clone)]
pub enum BatchEntry<'a> {
    /// A regular JSON-RPC call.
    Call(Request<'a>),
    /// A JSON-RPC notification.
    Notification(Notification<'a>),
}

impl<'a> BatchEntry<'a> {
    /// Get the method name of the batch entry.
    pub fn method_name(&self) -> &str {
        match self {
            BatchEntry::Call(req) => req.method_name(),
            BatchEntry::Notification(n) => n.method_name(),
        }
    }

    /// Get the params of the batch entry (may be empty).
    pub fn params(&self) -> Option<&Cow<'a, RawValue>> {
        match self {
            BatchEntry::Call(req) => req.params.as_ref(),
            BatchEntry::Notification(n) => n.params.as_ref(),
        }
    }
}

/// Represent a JSON-RPC service that can process JSON-RPC calls, notifications, and batch requests.
///
/// This trait is similar to [`tower::Service`] but it's specialized for JSON-RPC operations.
pub trait RpcServiceT {
    /// Processes a single JSON-RPC call, which may be a subscription or regular call.
    fn call<'a>(&self, request: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a;

    /// Processes multiple JSON-RPC calls at once, similar to `RpcServiceT::call`.
    ///
    /// This method wraps `RpcServiceT::call` and `RpcServiceT::notification`,
    /// but the root RPC service does not inherently recognize custom implementations
    /// of these methods.
    ///
    /// As a result, if you have custom logic for individual calls or notifications,
    /// you must duplicate that implementation in this method or no middleware will be applied
    /// for calls inside the batch.
    fn batch<'a>(&self, requests: Batch<'a>) -> impl Future<Output = MethodResponse> + Send + 'a;

    /// Similar to `RpcServiceT::call` but processes a JSON-RPC notification.
    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = MethodResponse> + Send + 'a;
}

/// Similar to [`tower::ServiceBuilder`] but doesn't
/// support any tower middleware implementations.
#[derive(Debug, Clone)]
pub struct RpcServiceBuilder<L>(tower::ServiceBuilder<L>);

impl Default for RpcServiceBuilder<Identity> {
    fn default() -> Self {
        RpcServiceBuilder(tower::ServiceBuilder::new())
    }
}

impl RpcServiceBuilder<Identity> {
    /// Create a new [`RpcServiceBuilder`].
    pub fn new() -> Self {
        Self(tower::ServiceBuilder::new())
    }
}

impl<L> RpcServiceBuilder<L> {
    /// Optionally add a new layer `T` to the [`RpcServiceBuilder`].
    ///
    /// See the documentation for [`tower::ServiceBuilder::option_layer`] for more details.
    pub fn option_layer<T>(
        self,
        layer: Option<T>,
    ) -> RpcServiceBuilder<Stack<layer::Either<T, Identity>, L>> {
        let layer = if let Some(layer) = layer {
            layer::Either::Left(layer)
        } else {
            layer::Either::Right(Identity::new())
        };
        RpcServiceBuilder(self.0.layer(layer))
    }

    /// Add a new layer `T` to the [`RpcServiceBuilder`].
    ///
    /// See the documentation for [`tower::ServiceBuilder::layer`] for more details.
    pub fn layer<T>(self, layer: T) -> RpcServiceBuilder<Stack<T, L>> {
        RpcServiceBuilder(self.0.layer(layer))
    }

    /// Add a [`tower::Layer`] built from a function that accepts a service and returns another
    /// service.
    ///
    /// See the documentation for [`tower::ServiceBuilder::layer_fn`] for more details.
    pub fn layer_fn<F>(self, f: F) -> RpcServiceBuilder<Stack<LayerFn<F>, L>> {
        RpcServiceBuilder(self.0.layer_fn(f))
    }

    /// Wrap the service `S` with the middleware.
    pub fn service<S>(&self, service: S) -> L::Service
    where
        L: tower::Layer<S>,
    {
        self.0.service(service)
    }
}

/// Response which may be ready or a future.
#[derive(Debug)]
#[pin_project]
pub struct ResponseFuture<F, R>(#[pin] futures_util::future::Either<F, std::future::Ready<R>>);

impl<F, R> ResponseFuture<F, R> {
    /// Returns a future that resolves to a response.
    pub fn future(f: F) -> ResponseFuture<F, R> {
        ResponseFuture(Either::Left(f))
    }

    /// Return a response which is already computed.
    pub fn ready(response: R) -> ResponseFuture<F, R> {
        ResponseFuture(Either::Right(std::future::ready(response)))
    }
}

impl<F, R> Future for ResponseFuture<F, R>
where
    F: Future<Output = R>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().0.poll(cx) {
            Poll::Ready(rp) => Poll::Ready(rp),
            Poll::Pending => Poll::Pending,
        }
    }
}
