//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;

/// Implements [`EthCall`](crate::eth::api::EthCall) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! eth_call_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::eth::api::EthCall for $network_api where
            Self: $crate::eth::api::Call + $crate::eth::api::LoadPendingBlock
        {
        }
    };
}

/// Implements [`Call`](crate::eth::api::Call) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! call_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::eth::api::Call for $network_api
        where
            Self: $crate::eth::api::LoadState + $crate::eth::api::SpawnBlocking,
            EvmConfig: reth_evm::ConfigureEvm,
        {
            #[inline]
            fn call_gas_limit(&self) -> u64 {
                self.inner.gas_cap()
            }

            #[inline]
            fn evm_config(&self) -> &impl reth_evm::ConfigureEvm {
                self.inner.evm_config()
            }
        }
    };
}

eth_call_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
call_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
