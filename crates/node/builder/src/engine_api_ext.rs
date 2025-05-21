//! [`EngineApiBuilder`] callback wrapper

use crate::rpc::EngineApiBuilder;
use eyre::Result;
use reth_node_api::{AddOnsContext, FullNodeComponents};
use reth_rpc_api::IntoEngineApiRpcModule;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Provides access to an `EngineApi` instance with a callback
#[derive(Debug)]
pub struct EngineApiFn<B, F> {
    /// The inner builder
    inner: B,
    /// Callback function to execute with the built API
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
    B::EngineApi: IntoEngineApiRpcModule + Send + Sync + 'static,
    F: FnOnce(&B::EngineApi) + Send + Sync + 'static,
{
    type EngineApi = B::EngineApi;

    async fn build_engine_api(mut self, ctx: &AddOnsContext<'_, N>) -> Result<Self::EngineApi> {
        let api = self.inner.build_engine_api(ctx).await?;

        if let Some(callback) = self.callback.take() {
            callback(&api);
        }

        Ok(api)
    }
}

/// Extension trait for `EngineApiBuilder` to add sender functionality
pub trait EngineApiBuilderExt<N>: EngineApiBuilder<N> + Sized
where
    N: FullNodeComponents,
    Self::EngineApi: IntoEngineApiRpcModule + Send + Sync + 'static,
{
    /// Wraps this builder to send the built API through a oneshot channel
    fn with_sender(
        self,
        sender: oneshot::Sender<Arc<dyn IntoEngineApiRpcModule + Send + Sync>>,
    ) -> EngineApiFn<Self, impl FnOnce(Self::EngineApi) + Send + Sync + 'static> {
        EngineApiFn::new(self, move |api| {
            let arc_api: Arc<dyn IntoEngineApiRpcModule + Send + Sync> = Arc::new(api);
            let _ = sender.send(arc_api);
        })
    }

    /// Wraps this builder with a custom callback
    fn with_callback<F>(self, callback: F) -> EngineApiFn<Self, F>
    where
        F: FnOnce(Self::EngineApi) + Send + Sync + 'static,
    {
        EngineApiFn::new(self, callback)
    }
}

impl<N, B> EngineApiBuilderExt<N> for B
where
    B: EngineApiBuilder<N>,
    N: FullNodeComponents,
    B::EngineApi: IntoEngineApiRpcModule + Send + Sync + 'static,
{
}
