//! Loads and formats OP block RPC response.   

use reth_node_api::FullNodeComponents;
use reth_provider::{BlockReaderIdExt, HeaderProvider};
use reth_rpc_eth_api::{
    helpers::{
        EthApiSpec, EthBlocks, LoadBlock, LoadPendingBlock, LoadTransaction,
        SpawnBlocking,
    },
};
use reth_rpc_eth_types::{EthStateCache};
use crate::error::TelosEthApiError;
use crate::eth::TelosEthApi;

impl<N> EthBlocks for TelosEthApi<N>
where
    Self: LoadBlock + EthApiSpec + LoadTransaction,
    Self::Error: From<TelosEthApiError>,
    N: FullNodeComponents,
{
    #[inline]
    fn provider(&self) -> impl HeaderProvider {
        self.inner.provider()
    }
}

impl<N> LoadBlock for TelosEthApi<N>
where
    Self: LoadPendingBlock + SpawnBlocking,
    N: FullNodeComponents,
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
