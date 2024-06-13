//! Contains RPC handler implementations specific to tracing.

use crate::EthApi;

/// Implements [`Trace`](crate::eth::api::Trace) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! trace_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::eth::api::Trace for $network_api
        where
            Self: $crate::eth::api::LoadState,
            EvmConfig: reth_evm::ConfigureEvm,
        {
            #[inline]
            fn evm_config(&self) -> &impl reth_evm::ConfigureEvm {
                self.inner.evm_config()
            }
        }
    };
}

trace_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
