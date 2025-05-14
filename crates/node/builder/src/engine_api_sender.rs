//! EngineApiBuilder providing access to the built Api through a channel.

use crate::rpc::EngineApiBuilder;
use eyre::Result;
use reth_node_api::{AddOnsContext, FullNodeComponents};
use reth_rpc_api::IntoEngineApiRpcModule;
use tokio::sync::oneshot;

/// Builder creating an `EngineApiBuilder` and sends a copy through a oneshot channel.
#[derive(Debug)]
pub struct EngineApiSender<B, T: Clone + Send + Sync + 'static> {
    /// The inner builder
    inner: B,
    /// The sender to send the built `EngineApi` instance
    sender: Option<oneshot::Sender<T>>,
}

impl<B, T: Clone + Send + Sync + 'static> EngineApiSender<B, T> {
    /// Create a new `EngineApi` sender
    pub const fn new(inner: B, sender: oneshot::Sender<T>) -> Self {
        Self { inner, sender: Some(sender) }
    }
}

impl<N, B, T> EngineApiBuilder<N> for EngineApiSender<B, T>
where
    B: EngineApiBuilder<N>,
    N: FullNodeComponents,
    B::EngineApi: IntoEngineApiRpcModule + Clone + Send + Sync + 'static,
    T: From<B::EngineApi> + Clone + Send + Sync + 'static,
{
    type EngineApi = B::EngineApi;

    async fn build_engine_api(mut self, ctx: &AddOnsContext<'_, N>) -> Result<Self::EngineApi> {
        let api = self.inner.build_engine_api(ctx).await?;

        if let Some(sender) = self.sender.take() {
            let _ = sender.send(T::from(api.clone()));
        }

        Ok(api)
    }
}
