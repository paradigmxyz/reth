//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use reth_evm::ConfigureEvm;

use crate::eth::{
    api::{Call, EthCall},
    EthApi,
};

impl<Provider, Pool, Network, EvmConfig> Call for EthApi<Provider, Pool, Network, EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }
}

impl<Provider, Pool, Network, EvmConfig> EthCall for EthApi<Provider, Pool, Network, EvmConfig> where
    Self: Call
{
}
