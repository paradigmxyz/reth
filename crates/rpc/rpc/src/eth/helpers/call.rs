//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use reth_evm::ConfigureEvmGeneric;
use reth_rpc_eth_api::helpers::{Call, EthCall, LoadPendingBlock, LoadState, SpawnBlocking};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthCall for EthApi<Provider, Pool, Network, EvmConfig> where
    Self: Call + LoadPendingBlock
{
}

impl<Provider, Pool, Network, EvmConfig> Call for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState + SpawnBlocking,
    EvmConfig: ConfigureEvmGeneric,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvmGeneric {
        self.inner.evm_config()
    }
}
