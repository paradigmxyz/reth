//! EngineApiBuilder providing access to the built Api through a channel.

use crate::rpc::EngineApiBuilder;
use eyre::Result;
use reth_node_api::{AddOnsContext, FullNodeComponents};
use reth_rpc_api::IntoEngineApiRpcModule;
use tokio::sync::oneshot;

/// Builder that wraps an `EngineApiBuilder` and executes a callback with the built API.
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
    B::EngineApi: IntoEngineApiRpcModule + Clone + Send + Sync + 'static,
    F: FnOnce(B::EngineApi) + Send + Sync + 'static,
{
    type EngineApi = B::EngineApi;

    async fn build_engine_api(mut self, ctx: &AddOnsContext<'_, N>) -> Result<Self::EngineApi> {
        let api = self.inner.build_engine_api(ctx).await?;

        if let Some(callback) = self.callback.take() {
            callback(api.clone());
        }

        Ok(api)
    }
}

/// Creates an [`EngineApiFn`] that sends the API through a oneshot channel
pub fn with_sender<N, B, T>(
    inner: B,
    sender: oneshot::Sender<T>,
) -> EngineApiFn<B, impl FnOnce(B::EngineApi) + Send + Sync + 'static>
where
    B: EngineApiBuilder<N>,
    N: FullNodeComponents,
    T: From<B::EngineApi> + Send + 'static,
{
    EngineApiFn::new(inner, move |api| {
        let _ = sender.send(T::from(api));
    })
}
