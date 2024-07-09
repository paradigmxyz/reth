use std::{marker::PhantomData, pin::Pin, sync::Arc};

use auto_impl::auto_impl;
use derive_more::Deref;
use futures::Future;
use reth_beacon_consensus::FullBlockchainTreeEngine;
use reth_blockchain_tree::BlockchainTreeConfig;
use reth_chainspec::Head;
use reth_consensus::Consensus;
use reth_network_p2p::{headers::client::HeadersClient, BodiesClient};
use reth_node_api::{
    EngineComponent, FullNodeComponents, FullNodeComponentsExt, FullNodeTypes, PipelineComponent,
    RpcComponent,
};
use reth_provider::ProviderFactory;
use reth_stages::MetricEvent;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    common::{Attached, InitializedComponents, LaunchContextWith, WithConfigs},
    hooks::OnComponentsInitializedHook,
    rpc::{ExtendRpcModules, OnRpcStarted, RpcHooks},
    NodeAdapterExt,
};

/// Type alias for extension component build context, holds the initialized core components.
pub type ExtBuilderContext<'a, Node: FullNodeComponentsExt> =
    LaunchContextWith<Attached<WithConfigs, &'a mut DynInitializedComponentsExt<Node>>>;

pub type DynInitializedComponentsExt<N: FullNodeComponentsExt> =
    Box<dyn InitializedComponentsExt<Node = N, Core = DynInitializedComponents<N>>>;

pub type DynInitializedComponents<N: FullNodeComponentsExt> =
    Box<dyn InitializedComponents<Node = N::Core, BlockchainTree = N::Tree>>;

/// A type that knows how to build the transaction pool.
pub trait NodeComponentsBuilderExt: Send {
    type Output: FullNodeComponentsExt;

    /// Creates the transaction pool.
    fn build(
        self,
        stage: Box<dyn StageExtComponentsBuild<Node = Self::Output>>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<Self::Output>> + Send>>;
}

pub struct NodeComponentsExtBuild<N, BT, C> {
    _marker: PhantomData<(N, BT, C)>,
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
        mut stage: Box<dyn StageExtComponentsBuild<Node = Self::Output>>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<Self::Output>> + Send>> {
        Box::pin(async move {
            if let Some(builder) = stage.build_pipeline() {
                builder.await?
            }
            if let Some(builder) = stage.build_engine() {
                builder.await?
            }
            if let Some(builder) = stage.build_rpc() {
                builder.await?
            }

            Ok(NodeAdapterExt {
                core: stage.components().core().node().clone(),
                tree: None,
                pipeline: None,
                engine: None,
                rpc: stage.components_mut().rpc_mut().take(),
            })
        })
    }
}

/// Staging environment for building extension components. Allows to control when components are
/// built, w.r.t. their inter-dependencies.
pub trait StageExtComponentsBuild: Send {
    type Node: FullNodeComponentsExt;

    fn components(&self) -> &DynInitializedComponentsExt<Self::Node>;

    fn components_mut(&mut self) -> &mut DynInitializedComponentsExt<Self::Node>;

    fn ctx_builder(&mut self, b: Box<dyn ExtComponentCtxBuilder<Self::Node>>);

    fn pipeline_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<Self::Node>>);

    fn engine_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<Self::Node>>);

    fn rpc_builder(&mut self, b: Box<dyn OnComponentsInitializedHook<Self::Node>>);

    /// Sets the hook that is run to configure the rpc modules.
    fn extend_rpc_modules(
        &mut self,
        hook: Box<dyn ExtendRpcModules<<Self::Node as FullNodeComponentsExt>::Core>>,
    ) {
        _ = self
            .components_mut()
            .rpc_add_ons_mut()
            .get_or_insert(RpcHooks::new())
            .set_extend_rpc_modules(hook)
    }

    /// Sets the hook that is run once the rpc server is started.
    fn on_rpc_started(
        &mut self,
        hook: Box<dyn OnRpcStarted<<Self::Node as FullNodeComponentsExt>::Core>>,
    ) {
        _ = self
            .components_mut()
            .rpc_add_ons_mut()
            .get_or_insert(RpcHooks::new())
            .set_on_rpc_started(hook)
    }

    fn build_ctx(&mut self) -> ExtBuilderContext<'_, Self::Node>;

    fn build_pipeline(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>>;

    fn build_engine(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>>;

    fn build_rpc(&mut self) -> Option<Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>>;
}

#[allow(missing_debug_implementations)]
pub struct ExtComponentsBuildStage<N: FullNodeComponentsExt> {
    pub core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
    pub components: DynInitializedComponentsExt<N>,
    pub ctx_builder: Box<dyn ExtComponentCtxBuilder<N>>,
    pub pipeline_builder: Option<Box<dyn OnComponentsInitializedHook<N>>>,
    pub engine_builder: Option<Box<dyn OnComponentsInitializedHook<N>>>,
    pub rpc_builder: Option<Box<dyn OnComponentsInitializedHook<N>>>,
    pub rpc_add_ons: Option<RpcHooks<N::Core>>,
}

impl<N: FullNodeComponentsExt> ExtComponentsBuildStage<N> {
    pub fn new<B, C>(
        core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
        ctx_builder: B,
        components: C,
    ) -> Self
    where
        B: ExtComponentCtxBuilder<N> + 'static,
        C: InitializedComponentsExt<Node = N, Core = DynInitializedComponents<N>> + 'static,
    {
        Self {
            core_ctx,
            ctx_builder: Box::new(ctx_builder),
            pipeline_builder: None,
            engine_builder: None,
            rpc_builder: None,
            rpc_add_ons: None,
            components: Box::new(components),
        }
    }
}

impl<N> StageExtComponentsBuild for ExtComponentsBuildStage<N>
where
    N: FullNodeComponentsExt + 'static,
    N::Core: FullNodeComponents,
    N::Tree: FullBlockchainTreeEngine + Clone + 'static,
    N::Pipeline: PipelineComponent + Send + Sync + Unpin + Clone + 'static,
    N::Engine: EngineComponent<N::Core> + 'static,
    N::Rpc: RpcComponent<N::Core> + 'static,
{
    type Node = N;

    fn components(&self) -> &DynInitializedComponentsExt<Self::Node> {
        &self.components
    }

    fn components_mut(&mut self) -> &mut DynInitializedComponentsExt<Self::Node> {
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
}

pub trait ExtComponentCtxBuilder<N: FullNodeComponentsExt>: Send {
    fn build_ctx<'a>(
        &self,
        core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
        components: &'a mut DynInitializedComponentsExt<N>,
    ) -> ExtBuilderContext<'a, N>;
}

impl<F, N> ExtComponentCtxBuilder<N> for F
where
    N: FullNodeComponentsExt,
    F: Fn(
            LaunchContextWith<Attached<WithConfigs, ()>>,
            &mut DynInitializedComponentsExt<N>,
        ) -> ExtBuilderContext<'_, N>
        + Send,
{
    fn build_ctx<'a>(
        &self,
        core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
        components: &'a mut DynInitializedComponentsExt<N>,
    ) -> ExtBuilderContext<'a, N> {
        self(core_ctx, components)
    }
}

pub fn build_ctx<'a, N: FullNodeComponentsExt>(
    core_ctx: LaunchContextWith<Attached<WithConfigs, ()>>,
    components: &'a mut DynInitializedComponentsExt<N>,
) -> ExtBuilderContext<'a, N> {
    ExtBuilderContext {
        inner: core_ctx.inner.clone(),
        attachment: core_ctx.attachment.clone_left(components),
    }
}

/// Builds extensions components.
#[auto_impl(&mut, Box)]
pub trait InitializedComponentsExt: Send {
    type Node: FullNodeComponentsExt;
    type Core: InitializedComponents<
        Node = <Self::Node as FullNodeComponentsExt>::Core,
        BlockchainTree = <Self::Node as FullNodeComponentsExt>::Tree,
    >;

    fn core(&self) -> &Self::Core;
    fn pipeline(&self) -> Option<&<Self::Node as FullNodeComponentsExt>::Pipeline>;
    fn engine(&self) -> Option<&<Self::Node as FullNodeComponentsExt>::Engine>;
    fn rpc(&self) -> Option<&<Self::Node as FullNodeComponentsExt>::Rpc>;

    fn pipeline_mut(&mut self) -> &mut Option<<Self::Node as FullNodeComponentsExt>::Pipeline>;
    fn engine_mut(&mut self) -> &mut Option<<Self::Node as FullNodeComponentsExt>::Engine>;
    fn rpc_mut(&mut self) -> &mut Option<<Self::Node as FullNodeComponentsExt>::Rpc>;
    fn rpc_add_ons_mut(
        &mut self,
    ) -> &mut Option<RpcHooks<<Self::Node as FullNodeComponentsExt>::Core>>;
}

impl<T> InitializedComponents for T
where
    T: InitializedComponentsExt,
{
    type Node = <T::Core as InitializedComponents>::Node;
    type BlockchainTree = <T::Core as InitializedComponents>::BlockchainTree;

    fn head(&self) -> Head {
        self.core().head()
    }

    fn provider_factory(&self) -> &ProviderFactory<<Self::Node as FullNodeTypes>::DB> {
        self.core().provider_factory()
    }

    fn blockchain_db(&self) -> &Self::BlockchainTree {
        self.core().blockchain_db()
    }

    fn consensus(&self) -> Arc<dyn Consensus> {
        self.core().consensus()
    }

    fn sync_metrics_tx(&self) -> UnboundedSender<MetricEvent> {
        self.core().sync_metrics_tx()
    }

    fn tree_config(&self) -> &BlockchainTreeConfig {
        self.core().tree_config()
    }

    fn node(&self) -> &Self::Node {
        self.core().node()
    }
}

#[allow(missing_debug_implementations)]
#[derive(Deref)]
pub struct WithComponentsExt<N: FullNodeComponentsExt> {
    #[deref]
    pub core: DynInitializedComponents<N>,
    pub pipeline: Option<N::Pipeline>,
    pub engine: Option<N::Engine>,
    pub engine_shutdown_rx: Option<<N::Engine as EngineComponent<N::Core>>::ShutdownRx>,
    pub rpc: Option<N::Rpc>,
    pub rpc_add_ons: Option<RpcHooks<N::Core>>,
}

impl<N: FullNodeComponentsExt> WithComponentsExt<N> {
    pub fn new<C>(core: C) -> Self
    where
        C: InitializedComponents<Node = N::Core, BlockchainTree = N::Tree> + 'static,
    {
        Self {
            core: Box::new(core),
            pipeline: None,
            engine: None,
            engine_shutdown_rx: None,
            rpc: None,
            rpc_add_ons: None,
        }
    }
}

impl<N: FullNodeComponentsExt> InitializedComponentsExt for WithComponentsExt<N> {
    type Node = N;
    type Core = DynInitializedComponents<N>;

    fn core(&self) -> &Self::Core {
        &self.core
    }

    fn pipeline(&self) -> Option<&N::Pipeline> {
        self.pipeline.as_ref()
    }
    fn engine(&self) -> Option<&N::Engine> {
        self.engine.as_ref()
    }
    fn rpc(&self) -> Option<&N::Rpc> {
        self.rpc.as_ref()
    }

    fn pipeline_mut(&mut self) -> &mut Option<N::Pipeline> {
        &mut self.pipeline
    }
    fn engine_mut(&mut self) -> &mut Option<N::Engine> {
        &mut self.engine
    }
    fn rpc_mut(&mut self) -> &mut Option<N::Rpc> {
        &mut self.rpc
    }
    fn rpc_add_ons_mut(&mut self) -> &mut Option<RpcHooks<N::Core>> {
        &mut self.rpc_add_ons
    }
}
