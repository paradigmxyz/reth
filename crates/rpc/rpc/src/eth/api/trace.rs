//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvm;

use crate::{eth::api::Trace, EthApi};

impl<Provider, Pool, Network, EvmConfig> Trace for EthApi<Provider, Pool, Network, EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }
}
