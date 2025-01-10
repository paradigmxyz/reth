//! Contains RPC handler implementations for fee history.

use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_provider::{BlockReader, BlockReaderIdExt, ChainSpecProvider, StateProviderFactory};
use reth_rpc_eth_api::helpers::{EthFees, LoadBlock, LoadFee};
use reth_rpc_eth_types::{FeeHistoryCache, GasPriceOracle};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthFees for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadFee,
    Provider: BlockReader,
{
}

impl<Provider, Pool, Network, EvmConfig> LoadFee for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadBlock<Provider = Provider>,
    Provider: BlockReaderIdExt
        + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
        + StateProviderFactory,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.fee_history_cache()
    }
}
