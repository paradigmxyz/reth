//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use alloy_consensus::Header;
use reth_evm::ConfigureEvm;
use reth_provider::{BlockReader, ProviderHeader};
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall, LoadPendingBlock, LoadState, SpawnBlocking},
    FullEthApiTypes,
};

impl<Provider, Pool, Network, EvmConfig> EthCall for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: EstimateCall + LoadPendingBlock + FullEthApiTypes,
    Provider: BlockReader,
{
}

impl<Provider, Pool, Network, EvmConfig> Call for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState<Evm: ConfigureEvm<Header = ProviderHeader<Self::Provider>>> + SpawnBlocking,
    EvmConfig: ConfigureEvm<Header = Header>,
    Provider: BlockReader,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.max_simulate_blocks()
    }
}

impl<Provider, Pool, Network, EvmConfig> EstimateCall for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Call,
    Provider: BlockReader,
{
}
