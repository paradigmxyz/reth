//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvmGeneric;
use reth_rpc_eth_api::helpers::{LoadState, Trace};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> Trace for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState,
    EvmConfig: ConfigureEvmGeneric,
{
    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvmGeneric {
        self.inner.evm_config()
    }
}
