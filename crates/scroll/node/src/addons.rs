use crate::{ScrollEngineValidator, ScrollEngineValidatorBuilder, ScrollStorage};
use reth_evm::{ConfigureEvm, EvmFactory, EvmFactoryFor};
use reth_node_api::{AddOnsContext, NodeAddOns};
use reth_node_builder::{
    rpc::{
        BasicEngineApiBuilder, EngineValidatorAddOn, EngineValidatorBuilder, EthApiBuilder,
        RethRpcAddOns, RpcAddOns, RpcHandle,
    },
    FullNodeComponents,
};
use reth_node_types::NodeTypes;
use reth_rpc_eth_types::error::FromEvmError;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_engine_primitives::ScrollEngineTypes;
use reth_scroll_evm::ScrollNextBlockEnvAttributes;
use reth_scroll_primitives::ScrollPrimitives;
use reth_scroll_rpc::{eth::ScrollEthApiBuilder, ScrollEthApi, ScrollEthApiError};
use revm::context::TxEnv;
use scroll_alloy_evm::ScrollTransactionIntoTxEnv;

/// Add-ons for the Scroll follower node.
#[derive(Debug)]
pub struct ScrollAddOns<N>
where
    N: FullNodeComponents,
    ScrollEthApiBuilder: EthApiBuilder<N>,
{
    /// Rpc add-ons responsible for launching the RPC servers and instantiating the RPC handlers
    /// and eth-api.
    pub rpc_add_ons: RpcAddOns<
        N,
        ScrollEthApiBuilder,
        ScrollEngineValidatorBuilder,
        BasicEngineApiBuilder<ScrollEngineValidatorBuilder>,
    >,
}

impl<N> Default for ScrollAddOns<N>
where
    N: FullNodeComponents<Types: NodeTypes<Primitives = ScrollPrimitives>>,
    ScrollEthApiBuilder: EthApiBuilder<N>,
{
    fn default() -> Self {
        Self::builder().build()
    }
}

impl<N> ScrollAddOns<N>
where
    N: FullNodeComponents<Types: NodeTypes<Primitives = ScrollPrimitives>>,
    ScrollEthApiBuilder: EthApiBuilder<N>,
{
    /// Build a [`ScrollAddOns`] using [`ScrollAddOnsBuilder`].
    pub fn builder() -> ScrollAddOnsBuilder {
        ScrollAddOnsBuilder::default()
    }
}

impl<N> NodeAddOns<N> for ScrollAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = ScrollChainSpec,
            Primitives = ScrollPrimitives,
            Storage = ScrollStorage,
            Payload = ScrollEngineTypes,
        >,
        Evm: ConfigureEvm<NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
    >,
    ScrollEthApiError: FromEvmError<N::Evm>,
    EvmFactoryFor<N::Evm>: EvmFactory<Tx = ScrollTransactionIntoTxEnv<TxEnv>>,
{
    type Handle = RpcHandle<N, ScrollEthApi<N>>;

    async fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Handle> {
        let Self { rpc_add_ons } = self;
        rpc_add_ons.launch_add_ons_with(ctx, |_, _, _| Ok(())).await
    }
}

impl<N> RethRpcAddOns<N> for ScrollAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = ScrollChainSpec,
            Primitives = ScrollPrimitives,
            Storage = ScrollStorage,
            Payload = ScrollEngineTypes,
        >,
        Evm: ConfigureEvm<NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
    >,
    ScrollEthApiError: FromEvmError<N::Evm>,
    EvmFactoryFor<N::Evm>: EvmFactory<Tx = ScrollTransactionIntoTxEnv<TxEnv>>,
{
    type EthApi = ScrollEthApi<N>;

    fn hooks_mut(&mut self) -> &mut reth_node_builder::rpc::RpcHooks<N, Self::EthApi> {
        self.rpc_add_ons.hooks_mut()
    }
}

impl<N> EngineValidatorAddOn<N> for ScrollAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = ScrollChainSpec,
            Primitives = ScrollPrimitives,
            Payload = ScrollEngineTypes,
        >,
    >,
    ScrollEthApiBuilder: EthApiBuilder<N>,
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
        N: FullNodeComponents<Types: NodeTypes<Primitives = ScrollPrimitives>>,
        ScrollEthApiBuilder: EthApiBuilder<N>,
    {
        ScrollAddOns {
            rpc_add_ons: RpcAddOns::new(
                ScrollEthApi::<N>::builder(),
                Default::default(),
                Default::default(),
            ),
        }
    }
}
