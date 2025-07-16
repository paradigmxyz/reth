//! Contains RPC handler implementations specific to blocks.

use reth_chainspec::ChainSpecProvider;
use reth_evm::ConfigureEvm;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock, SpawnBlocking},
    RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::{BlockReader, ProviderTx};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig, Rpc> EthBlocks
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: LoadBlock<
        Error = EthApiError,
        NetworkTypes = Rpc::Network,
        RpcConvert: RpcConvert<
            Primitives = Self::Primitives,
            Error = Self::Error,
            Network = Rpc::Network,
        >,
    >,
    Provider: BlockReader + ChainSpecProvider,
    Rpc: RpcConvert,
{
}

impl<Provider, Pool, Network, EvmConfig, Rpc> LoadBlock
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: LoadPendingBlock
        + SpawnBlocking
        + RpcNodeCoreExt<
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Primitives: NodePrimitives<SignedTx = ProviderTx<Self::Provider>>,
            Evm = EvmConfig,
        >,
    Provider: BlockReader,
    EvmConfig: ConfigureEvm<Primitives = <Self as RpcNodeCore>::Primitives>,
    Rpc: RpcConvert,
{
}
