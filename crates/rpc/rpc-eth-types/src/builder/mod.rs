//! `eth` namespace API builder types.

pub mod config;
pub mod ctx;

pub trait BoxedDynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi> {
    fn call(
        self: Box<Self>,
        args: &ctx::EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> EthApi;
}

pub trait DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>:
    BoxedDynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>
    + FnOnce(&ctx::EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>) -> EthApi
    + Send
{
    fn call(
        self,
        args: &ctx::EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> EthApi;
}

impl<F, Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>
    DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi> for F
where
    F: FnOnce(&ctx::EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>) -> EthApi
        + Send,
{
    fn call(
        self,
        args: &ctx::EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> EthApi {
        self(args)
    }
}

impl<T, Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>
    BoxedDynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi> for T
where
    T: DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>,
{
    fn call(
        self: Box<Self>,
        args: &ctx::EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
    ) -> EthApi {
        <Self as DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>>::call(
            *self, args,
        )
    }
}

//impl<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>
//    DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi>
//    for Box<dyn DynEthApiBuilder<Provider, Pool, EvmConfig, Network, Tasks, Events, EthApi> + '_>
//{
//    fn call(
//        self,
//        args: &ctx::EthApiBuilderCtx<Provider, Pool, EvmConfig, Network, Tasks, Events>,
//    ) -> EthApi {
//        self.call(args)
//    }
//}
