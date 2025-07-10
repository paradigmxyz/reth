//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use alloy_evm::block::BlockExecutorFactory;
use alloy_network::TransactionBuilder;
use reth_errors::ProviderError;
use reth_evm::{ConfigureEvm, EvmFactory, TxEnvFor};
use reth_node_api::NodePrimitives;
use reth_rpc_convert::{RpcConvert, RpcTypes};
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall, LoadPendingBlock, LoadState, SpawnBlocking},
    FromEvmError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt,
};
use reth_storage_api::{BlockReader, ProviderHeader, ProviderTx};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm::context::TxEnv;

impl<Provider, Pool, Network, EvmConfig, Rpc> EthCall
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: EstimateCall<NetworkTypes = Rpc>
        + LoadPendingBlock<NetworkTypes = Rpc>
        + FullEthApiTypes<NetworkTypes = Rpc>
        + RpcNodeCoreExt<
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Primitives: NodePrimitives<SignedTx = ProviderTx<Self::Provider>>,
            Evm = EvmConfig,
        >,
    EvmConfig: ConfigureEvm<Primitives = <Self as RpcNodeCore>::Primitives>,
    Provider: BlockReader,
    Rpc: alloy_network::Network + RpcTypes<TransactionRequest: TransactionBuilder<Rpc>>,
{
}

impl<Provider, Pool, Network, EvmConfig, Rpc> Call
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: LoadState<
            Evm: ConfigureEvm<
                BlockExecutorFactory: BlockExecutorFactory<EvmFactory: EvmFactory<Tx = TxEnv>>,
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
            >,
            RpcConvert: RpcConvert<TxEnv = TxEnvFor<Self::Evm>, Network = Rpc>,
            NetworkTypes = Rpc,
            Error: FromEvmError<Self::Evm>
                       + From<<Self::RpcConvert as RpcConvert>::Error>
                       + From<ProviderError>,
        > + SpawnBlocking,
    Provider: BlockReader,
    Rpc: alloy_network::Network + RpcTypes<TransactionRequest: TransactionBuilder<Rpc>>,
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

impl<Provider, Pool, Network, EvmConfig, Rpc> EstimateCall
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: Call<NetworkTypes = Rpc>,
    Provider: BlockReader,
    Rpc: alloy_network::Network,
{
}
