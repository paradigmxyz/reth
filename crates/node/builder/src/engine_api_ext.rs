//! `EngineApiBuilder` callback wrapper
//!
//! Wraps an `EngineApiBuilder` to provide access to the built Engine API instance.

use crate::rpc::EngineApiBuilder;
use eyre::Result;
use reth_node_api::{AddOnsContext, FullNodeComponents};
use reth_rpc_api::IntoEngineApiRpcModule;

/// Provides access to an `EngineApi` instance with a callback
#[derive(Debug)]
pub struct EngineApiExt<B, F> {
    /// The inner builder that constructs the actual `EngineApi`
    inner: B,
    /// Optional callback function to execute with the built API
    callback: Option<F>,
}

impl<B, F> EngineApiExt<B, F> {
    /// Creates a new wrapper that calls `callback` when the API is built.
    pub const fn new(inner: B, callback: F) -> Self {
        Self { inner, callback: Some(callback) }
    }
}

impl<N, B, F> EngineApiBuilder<N> for EngineApiExt<B, F>
where
    B: EngineApiBuilder<N>,
    N: FullNodeComponents,
    B::EngineApi: IntoEngineApiRpcModule + Send + Sync + Clone + 'static,
    F: FnOnce(B::EngineApi) + Send + Sync + 'static,
{
    type EngineApi = B::EngineApi;

    /// Builds the `EngineApi` and executes the callback if present.
    async fn build_engine_api(mut self, ctx: &AddOnsContext<'_, N>) -> Result<Self::EngineApi> {
        let api = self.inner.build_engine_api(ctx).await?;

        if let Some(callback) = self.callback.take() {
            callback(api.clone());
        }

        Ok(api)
    }
}
