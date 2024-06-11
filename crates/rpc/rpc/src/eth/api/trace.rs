//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvm;

use crate::{
    eth::api::{LoadState, Trace},
    EthApi,
};

impl<Provider, Pool, Network, EvmConfig> Trace for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState,
    EvmConfig: ConfigureEvm,
{
    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }
}
