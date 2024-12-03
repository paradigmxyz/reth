//! Support for building a pending block with transactions from local view of mempool.

use alloy_consensus::Header;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ProviderTx, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    RpcNodeCore,
};
use reth_rpc_eth_types::PendingBlock;
use reth_transaction_pool::{PoolTransaction, TransactionPool};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking
        + RpcNodeCore<
            Provider: BlockReaderIdExt<
                Transaction = reth_primitives::TransactionSigned,
                Block = reth_primitives::Block,
                Receipt = reth_primitives::Receipt,
                Header = reth_primitives::Header,
            > + EvmEnvProvider
                          + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                          + StateProviderFactory,
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Evm: ConfigureEvm<Header = Header>,
        >,
{
    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock>> {
        self.inner.pending_block()
    }
}
