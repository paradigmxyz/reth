//! Support for building a pending block with transactions from local view of mempool.

use crate::EthApi;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_node_api::NodePrimitives;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{pending_block::PendingEnvBuilder, LoadPendingBlock, SpawnBlocking},
    types::RpcTypes,
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::PendingBlock;
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, ProviderBlock, ProviderHeader, ProviderReceipt, ProviderTx,
    StateProviderFactory,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

impl<Provider, Pool, Network, EvmConfig, Rpc> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: SpawnBlocking<
            NetworkTypes = Rpc::Network,
            Error: FromEvmError<Self::Evm>,
            RpcConvert: RpcConvert<Network = Rpc::Network>,
        > + RpcNodeCore<
            Provider: BlockReaderIdExt<Receipt = Provider::Receipt, Block = Provider::Block>
                          + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                          + StateProviderFactory,
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Evm = EvmConfig,
            Primitives: NodePrimitives<
                BlockHeader = ProviderHeader<Self::Provider>,
                SignedTx = ProviderTx<Self::Provider>,
                Receipt = ProviderReceipt<Self::Provider>,
                Block = ProviderBlock<Self::Provider>,
            >,
        >,
    Provider: BlockReader,
    EvmConfig: ConfigureEvm<Primitives = Self::Primitives>,
    Rpc: RpcConvert<
        Network: RpcTypes<Header = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>>,
    >,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>,
    > {
        self.inner.pending_block()
    }

    #[inline]
    fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
        self.inner.pending_env_builder()
    }
}
