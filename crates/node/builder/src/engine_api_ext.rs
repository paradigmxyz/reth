//! `EngineApiBuilder` callback wrapper
//!
//! Provides a wrapper around `EngineApiBuilder` that allows executing
//! callbacks when the Engine API is built.

use crate::rpc::EngineApiBuilder;
use eyre::Result;
use reth_node_api::{AddOnsContext, FullNodeComponents};
use reth_rpc_api::IntoEngineApiRpcModule;
use tokio::sync::oneshot;

/// Provides access to an `EngineApi` instance with a callback
#[derive(Debug)]
pub struct EngineApiFn<B, F> {
    /// The inner builder that constructs the actual `EngineApi`
    inner: B,
    /// Optional callback function to execute with the built API
    callback: Option<F>,
}

impl<B, F> EngineApiFn<B, F> {
    /// Create a new `EngineApiFn` with the given builder and callback
    pub const fn new(inner: B, callback: F) -> Self {
        Self { inner, callback: Some(callback) }
    }
}

impl<N, B, F> EngineApiBuilder<N> for EngineApiFn<B, F>
where
    B: EngineApiBuilder<N>,
    N: FullNodeComponents,
    B::EngineApi: IntoEngineApiRpcModule + Send + Sync + Clone + 'static,
    F: FnOnce(B::EngineApi) + Send + Sync + 'static,
{
    type EngineApi = B::EngineApi;

    // Builds the `EngineApi` and executes the callback if present.
    async fn build_engine_api(mut self, ctx: &AddOnsContext<'_, N>) -> Result<Self::EngineApi> {
        let api = self.inner.build_engine_api(ctx).await?;

        if let Some(callback) = self.callback.take() {
            callback(api.clone());
        }

        Ok(api)
    }
}

/// Extension trait providing methods for wrapping `EngineApiBuilder` with callbacks
pub trait EngineApiBuilderExt<N>: EngineApiBuilder<N> + Sized
where
    N: FullNodeComponents,
    Self::EngineApi: IntoEngineApiRpcModule + Send + Sync + Clone + 'static,
{
    /// Wraps this builder to send the built api through a oneshot channel
    fn with_sender(
        self,
        sender: oneshot::Sender<Self::EngineApi>,
    ) -> EngineApiFn<Self, impl FnOnce(Self::EngineApi) + Send + Sync + 'static>;

    /// Wraps the builder with a custom callback
    fn with_callback<F>(self, callback: F) -> EngineApiFn<Self, F>
    where
        F: FnOnce(Self::EngineApi) + Send + Sync + 'static;
}

impl<N, B> EngineApiBuilderExt<N> for B
where
    B: EngineApiBuilder<N>,
    N: FullNodeComponents,
    B::EngineApi: IntoEngineApiRpcModule + Send + Sync + Clone + 'static,
{
    fn with_sender(
        self,
        sender: oneshot::Sender<Self::EngineApi>,
    ) -> EngineApiFn<Self, impl FnOnce(Self::EngineApi) + Send + Sync + 'static> {
        EngineApiFn::new(self, move |api: Self::EngineApi| {
            let _ = sender.send(api);
        })
    }

    fn with_callback<F>(self, callback: F) -> EngineApiFn<Self, F>
    where
        F: FnOnce(Self::EngineApi) + Send + Sync + 'static,
    {
        EngineApiFn::new(self, callback)
    }
}
