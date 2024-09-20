//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use reth_evm::ConfigureEvm;
use reth_primitives::Header;
use reth_rpc_eth_api::helpers::{Call, EthCall, LoadPendingBlock, LoadState, SpawnBlocking};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthCall for EthApi<Provider, Pool, Network, EvmConfig> where
    Self: Call + LoadPendingBlock
{
}

impl<Provider, Pool, Network, EvmConfig> Call for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState + SpawnBlocking,
    EvmConfig: ConfigureEvm<Header = Header>,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.max_simulate_blocks()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm<Header = Header> {
        self.inner.evm_config()
    }
}
