//! Support for building a pending block with transactions from local view of mempool.

use crate::EthApi;
use reth_evm::ConfigureEvm;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{pending_block::PendingEnvBuilder, LoadPendingBlock, SpawnBlocking},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::PendingBlock;
use reth_storage_api::{BlockReader, ProviderBlock, ProviderReceipt};

impl<N, Rpc> LoadPendingBlock for EthApi<N, Rpc>
where
    Self: SpawnBlocking<
            NetworkTypes = Rpc::Network,
            Error: FromEvmError<Self::Evm>,
            RpcConvert: RpcConvert<Network = Rpc::Network>,
        > + RpcNodeCore,
    Rpc: RpcConvert,
{
    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock<Self::Primitives>>> {
        self.inner.pending_block()
    }

    #[inline]
    fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
        self.inner.pending_env_builder()
    }
}
