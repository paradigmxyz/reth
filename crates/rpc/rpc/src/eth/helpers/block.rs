//! Contains RPC handler implementations specific to blocks.

use reth_provider::{BlockReaderIdExt, HeaderProvider};
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock, SpawnBlocking},
    Transaction,
};
use reth_rpc_eth_types::EthStateCache;
use reth_rpc_types_compat::{BlockBuilder, TransactionBuilder};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthBlocks for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadBlock + BlockBuilder,
    Self::TxBuilder: TransactionBuilder<Transaction = Transaction<Self>>,
    Provider: HeaderProvider,
{
    #[inline]
    fn provider(&self) -> impl reth_provider::HeaderProvider {
        self.inner.provider()
    }
}

impl<Provider, Pool, Network, EvmConfig> LoadBlock for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadPendingBlock + SpawnBlocking,
    Provider: BlockReaderIdExt,
{
    #[inline]
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}
