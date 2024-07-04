use std::{marker::PhantomData, mem, pin::Pin, sync::Arc};

use futures::Future;
use reth_beacon_consensus::FullBlockchainTreeEngine;
use reth_blockchain_tree::BlockchainTreeConfig;
use reth_consensus::Consensus;
use reth_network_p2p::{headers::client::HeadersClient, BodiesClient};
use reth_node_api::{
    EngineComponent, FullNodeComponents, FullNodeComponentsExt, PipelineComponent, RpcComponent,
};
use reth_primitives::Head;
use reth_provider::ProviderFactory;
use reth_stages::MetricEvent;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    common::{Attached, InitializedComponents, LaunchContextWith, WithConfigs},
    components::NodeComponents,
    hooks::OnComponentsInitializedHook,
    rpc::OnRpcStarted,
    NodeAdapterExt,
};

/// Type alias for extension component build context, holds the initialized core components.
pub type ExtBuilderContext<'a, Node: FullNodeComponentsExt> =
    LaunchContextWith<Attached<WithConfigs, &'a mut Box<dyn InitializedComponentsExt<Node>>>>;

/// A type that knows how to build the transaction pool.
pub trait NodeComponentsBuilderExt: Send {
    type Output: FullNodeComponentsExt;

    /// Creates the transaction pool.
    fn build(
        self,
        stage: Box<
            dyn StageExtComponentsBuild<
                Self::Output,
                Components = Box<dyn InitializedComponentsExt<Self::Output>>,
            >,
        >,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<Self::Output>> + Send>>;
}

pub struct NodeComponentsExtBuild<N, BT, C> {
    _phantom: PhantomData<(N, BT, C)>,
}

impl<N, BT, C> NodeComponentsBuilderExt for NodeComponentsExtBuild<N, BT, C>
where
    N: FullNodeComponents + Clone,
    BT: FullBlockchainTreeEngine + Clone + 'static,
    C: HeadersClient + BodiesClient + Unpin + Clone + 'static,
{
    type Output = NodeAdapterExt<N, BT, C>;

    fn build(
        self,
        stage: Box<
            dyn StageExtComponentsBuild<
                Self::Output,
                Components = Box<dyn InitializedComponentsExt<Self::Output>>,
            >,
        >,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<Self::Output>> + Send>> {
        Box::pin(async move {
            let _pipeline = stage.build_pipeline().await;
            let _engine = stage.build_engine().await;
            let rpc = stage.build_rpc().await;

            NodeAdapterExt::new(stage.components_mut().node().clone())
        })
    }
}

/// Staging environment for building extension components. Allows to control when components are
/// built, w.r.t. their inter-dependencies.
pub trait StageExtComponentsBuild<N: FullNodeComponentsExt> {
    type Components: InitializedComponentsExt<N> + 'static;

    fn components_mut(&mut self) -> &mut Self::Components;

    fn engine_shutdown_rx(&mut self) -> <N::Engine as EngineComponent<N>>::ShutdownRx {
        if let Some(rx) = self.components_mut().engine_mut().map(|engine| engine.shutdown_rx_mut())
        {
            return mem::take(rx)
        }
        <N::Engine as EngineComponent<N>>::ShutdownRx::default()
    }

    fn ctx_builder(&mut self, b: Box<dyn ExtComponentCtxBuilder<N>>);

    fn pipeline_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<N>>);

    fn engine_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<N>>);

    fn rpc_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<N>>);

    fn build_ctx(&mut self) -> ExtBuilderContext<'_, N>;

    fn build_pipeline(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>>;

    fn build_engine(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>>;

    fn build_rpc(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>>;

    /// Sets the hook that is run once the rpc server is started.
    fn on_rpc_started(&mut self, hook: Box<dyn OnRpcStarted<N>>);
}

#[allow(missing_debug_implementations)]
pub struct ExtComponentsBuildStage<N: FullNodeComponentsExt> {
    pub core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
    pub components: Box<dyn InitializedComponentsExt<N>>,
    pub ctx_builder: Box<dyn ExtComponentCtxBuilder<N>>,
    pub pipeline_builder: Option<Box<dyn OnComponentsInitializedHook<N>>>,
    pub engine_builder: Option<Box<dyn OnComponentsInitializedHook<N>>>,
    pub rpc_builder: Option<Box<dyn OnComponentsInitializedHook<N>>>,
    pub rpc_add_ons: Vec<Box<dyn OnRpcStarted<N>>>,
}

impl<N: FullNodeComponentsExt> ExtComponentsBuildStage<N> {
    pub fn new<B, C>(
        core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
        ctx_builder: B,
        components: C,
    ) -> Self
    where
        C: InitializedComponentsExt<N> + 'static,
        B: ExtComponentCtxBuilder<N> + 'static,
    {
        Self {
            core_ctx,
            ctx_builder: Box::new(ctx_builder),
            pipeline_builder: None,
            engine_builder: None,
            rpc_builder: None,
            rpc_add_ons: vec![],
            components: Box::new(components),
        }
    }
}

impl<N> StageExtComponentsBuild<N> for ExtComponentsBuildStage<N>
where
    N: FullNodeComponentsExt + 'static,
    Box<(dyn InitializedComponentsExt<N> + 'static)>:
        InitializedComponents + InitializedComponentsExt<N>,
    N::Tree: FullBlockchainTreeEngine + Clone + 'static,
    N::Pipeline: PipelineComponent + Send + Sync + Unpin + Clone + 'static,
    N::Engine: EngineComponent<N> + 'static,
    N::Rpc: RpcComponent<N> + 'static,
{
    type Components = Box<dyn InitializedComponentsExt<N>>;

    fn components_mut(&mut self) -> &mut Self::Components {
        &mut self.components
    }

    fn ctx_builder(&mut self, b: Box<dyn ExtComponentCtxBuilder<N>>) {
        self.ctx_builder = b
    }

    fn pipeline_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<N>>) {
        self.pipeline_builder = Some(b)
    }

    fn engine_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<N>>) {
        self.engine_builder = Some(b)
    }

    fn rpc_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<N>>) {
        self.rpc_builder = Some(b)
    }

    fn build_ctx(&mut self) -> ExtBuilderContext<'_, N> {
        let ctx_builder = &self.ctx_builder;
        let core_ctx = self.core_ctx.clone();
        let components = &mut self.components;
        ctx_builder.build_ctx(core_ctx, components)
    }

    fn build_pipeline(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>> {
        let pipeline_builder = self.pipeline_builder.take()?;
        let ctx = self.build_ctx();
        Some(pipeline_builder.on_event(ctx))
    }

    fn build_engine(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>> {
        let engine_builder = self.engine_builder.take()?;
        let ctx = self.build_ctx();
        Some(engine_builder.on_event(ctx))
    }

    fn build_rpc(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>> {
        let rpc_builder = self.rpc_builder.take()?;
        let ctx = self.build_ctx();
        Some(rpc_builder.on_event(ctx))
    }

    fn on_rpc_started(&mut self, hook: Box<dyn OnRpcStarted<N>>) {
        self.rpc_add_ons.push(hook);
    }
}

pub trait ExtComponentCtxBuilder<N: FullNodeComponentsExt>: Send {
    fn build_ctx<'a>(
        &self,
        core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
        components: &'a mut Box<dyn InitializedComponentsExt<N>>,
    ) -> ExtBuilderContext<'a, N>;
}

impl<F, N> ExtComponentCtxBuilder<N> for F
where
    N: FullNodeComponentsExt,
    F: Fn(
            LaunchContextWith<Attached<WithConfigs, ()>>,
            &mut Box<dyn InitializedComponentsExt<N>>,
        ) -> ExtBuilderContext<'_, N>
        + Send,
{
    fn build_ctx<'a>(
        &self,
        core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
        components: &'a mut Box<dyn InitializedComponentsExt<N>>,
    ) -> ExtBuilderContext<'a, N> {
        self(core_ctx, components)
    }
}

pub fn build_ctx<'a, N: FullNodeComponentsExt>(
    core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
    components: &'a mut Box<dyn InitializedComponentsExt<N>>,
) -> ExtBuilderContext<'a, N> {
    ExtBuilderContext {
        inner: core_ctx.inner.clone(),
        attachment: core_ctx.attachment.clone_left(components),
    }
}

/// Builds extensions components.
pub trait InitializedComponentsExt<N: FullNodeComponentsExt>:
    InitializedComponents<Node = N, BlockchainTree = N::Tree>
{
    fn pipeline(&self) -> Option<&<Self::Node as FullNodeComponentsExt>::Pipeline>;
    fn engine(&self) -> Option<&<Self::Node as FullNodeComponentsExt>::Engine>;
    fn rpc(&self) -> Option<&<Self::Node as FullNodeComponentsExt>::Rpc>;

    fn pipeline_mut(&mut self) -> Option<&mut <Self::Node as FullNodeComponentsExt>::Pipeline>;
    fn engine_mut(&mut self) -> Option<&mut <Self::Node as FullNodeComponentsExt>::Engine>;
    fn rpc_mut(&mut self) -> Option<&mut <Self::Node as FullNodeComponentsExt>::Rpc>;
}

impl<N: FullNodeComponentsExt> InitializedComponentsExt<N> for WithComponentsExt<N> {
    fn pipeline(&self) -> Option<&N::Pipeline> {
        self.pipeline.as_ref()
    }
    fn engine(&self) -> Option<&N::Engine> {
        self.engine.as_ref()
    }
    fn rpc(&self) -> Option<&N::Rpc> {
        self.rpc.as_ref()
    }

    fn pipeline_mut(&mut self) -> Option<&mut N::Pipeline> {
        self.pipeline.as_mut()
    }
    fn engine_mut(&mut self) -> Option<&mut N::Engine> {
        self.engine.as_mut()
    }
    fn rpc_mut(&mut self) -> Option<&mut N::Rpc> {
        self.rpc.as_mut()
    }
}

#[allow(missing_debug_implementations)]
pub struct WithComponentsExt<N: FullNodeComponentsExt> {
    pub core: Box<dyn InitializedComponents<Node = N, BlockchainTree = N::Tree>>,
    pub pipeline: Option<N::Pipeline>,
    pub engine: Option<N::Engine>,
    pub engine_shutdown_rx: Option<<N::Engine as EngineComponent<N>>::ShutdownRx>,
    pub rpc: Option<N::Rpc>,
}

impl<N: FullNodeComponentsExt> WithComponentsExt<N> {
    pub fn new<C>(core: C) -> Self
    where
        C: InitializedComponents<Node = N, BlockchainTree = N::Tree> + 'static,
    {
        Self {
            core: Box::new(core),
            pipeline: None,
            engine: None,
            engine_shutdown_rx: None,
            rpc: None,
        }
    }
}

impl<N: FullNodeComponentsExt> InitializedComponents for WithComponentsExt<N> {
    type Node = N;
    type BlockchainTree = N::Tree;

    /// Returns the current head block.
    fn head(&self) -> Head {
        self.core.head()
    }

    /// Returns the configured database provider.
    fn provider_factory(&self) -> &ProviderFactory<N::DB> {
        self.core.provider_factory()
    }

    /// Returns a reference to the blockchain provider.
    fn blockchain_db(&self) -> &N::Tree {
        self.core.blockchain_db()
    }

    /// Returns the configured `Consensus`.
    fn consensus(&self) -> Arc<dyn Consensus> {
        self.core.consensus()
    }

    /// Returns the metrics sender.
    fn sync_metrics_tx(&self) -> UnboundedSender<MetricEvent> {
        self.core.sync_metrics_tx()
    }

    /// Returns a reference to the `BlockchainTreeConfig`.
    fn tree_config(&self) -> &BlockchainTreeConfig {
        self.core.tree_config()
    }

    /// Returns the core components.
    fn node(&self) -> &Self::Node {
        self.core.node()
    }
}
