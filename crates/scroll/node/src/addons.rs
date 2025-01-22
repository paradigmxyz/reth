use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_node_api::{AddOnsContext, NodeAddOns};
use reth_node_builder::{
    rpc::{EngineValidatorAddOn, EngineValidatorBuilder, RethRpcAddOns, RpcAddOns, RpcHandle},
    FullNodeComponents,
};
use reth_node_types::{NodeTypes, NodeTypesWithEngine};
use reth_primitives::EthPrimitives;

use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_rpc::ScrollEthApi;

use crate::{ScrollEngineValidator, ScrollEngineValidatorBuilder, ScrollStorage};

/// Add-ons for the Scroll follower node.
#[derive(Debug)]
pub struct ScrollAddOns<N: FullNodeComponents> {
    /// Rpc add-ons responsible for launching the RPC servers and instantiating the RPC handlers
    /// and eth-api.
    pub rpc_add_ons: RpcAddOns<N, ScrollEthApi<N>, ScrollEngineValidatorBuilder>,
}

impl<N: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>> Default
    for ScrollAddOns<N>
{
    fn default() -> Self {
        Self::builder().build()
    }
}

impl<N: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>> ScrollAddOns<N> {
    /// Build a [`ScrollAddOns`] using [`ScrollAddOnsBuilder`].
    pub fn builder() -> ScrollAddOnsBuilder {
        ScrollAddOnsBuilder::default()
    }
}

impl<N> NodeAddOns<N> for ScrollAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<
            ChainSpec = ScrollChainSpec,
            Primitives = EthPrimitives,
            Storage = ScrollStorage,
            Engine = EthEngineTypes,
        >,
    >,
{
    type Handle = RpcHandle<N, ScrollEthApi<N>>;

    async fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Handle> {
        let Self { rpc_add_ons } = self;
        rpc_add_ons.launch_add_ons_with(ctx, |_, _| Ok(())).await
    }
}

impl<N> RethRpcAddOns<N> for ScrollAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<
            ChainSpec = ScrollChainSpec,
            Primitives = EthPrimitives,
            Storage = ScrollStorage,
            Engine = EthEngineTypes,
        >,
    >,
{
    type EthApi = ScrollEthApi<N>;

    fn hooks_mut(&mut self) -> &mut reth_node_builder::rpc::RpcHooks<N, Self::EthApi> {
        self.rpc_add_ons.hooks_mut()
    }
}

impl<N> EngineValidatorAddOn<N> for ScrollAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<
            ChainSpec = ScrollChainSpec,
            Primitives = EthPrimitives,
            Engine = EthEngineTypes,
        >,
    >,
{
    type Validator = ScrollEngineValidator;

    async fn engine_validator(&self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        ScrollEngineValidatorBuilder.build(ctx).await
    }
}

/// A regular scroll evm and executor builder.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct ScrollAddOnsBuilder {}

impl ScrollAddOnsBuilder {
    /// Builds an instance of [`ScrollAddOns`].
    pub fn build<N>(self) -> ScrollAddOns<N>
    where
        N: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
    {
        ScrollAddOns {
            rpc_add_ons: RpcAddOns::new(
                move |ctx| ScrollEthApi::<N>::builder().build(ctx),
                Default::default(),
            ),
        }
    }
}
