//! Contains RPC handler implementations for fee history.

use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_node_api::FullNodeComponents;
use reth_rpc_eth_api::helpers::{EthFees, LoadBlock, LoadFee};
use reth_rpc_eth_types::{FeeHistoryCache, GasPriceOracle};
use reth_storage_api::{BlockReader, BlockReaderIdExt, StateProviderFactory};

use crate::EthApi;

impl<Components: FullNodeComponents> EthFees for EthApi<Components>
where
    Self: LoadFee,
    Components::Provider: BlockReader,
{
}

impl<Components: FullNodeComponents> LoadFee for EthApi<Components>
where
    Self: LoadBlock<Provider = Components::Provider>,
    Components::Provider: BlockReaderIdExt
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
