//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use alloy_evm::block::BlockExecutorFactory;
use reth_errors::ProviderError;
use reth_evm::{ConfigureEvm, EvmFactory, TxEnvFor};
use reth_node_api::NodePrimitives;
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall, LoadPendingBlock, LoadState, SpawnBlocking},
    FromEvmError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_types_compat::TransactionCompat;
use reth_storage_api::{BlockReader, ProviderHeader, ProviderTx};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm::context::TxEnv;

impl<Provider, Pool, Network, EvmConfig> EthCall for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: EstimateCall
        + LoadPendingBlock
        + FullEthApiTypes
        + RpcNodeCoreExt<
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Primitives: NodePrimitives<SignedTx = ProviderTx<Self::Provider>>,
            Evm = EvmConfig,
        >,
    EvmConfig: ConfigureEvm<Primitives = <Self as RpcNodeCore>::Primitives>,
    Provider: BlockReader,
{
}

impl<Provider, Pool, Network, EvmConfig> Call for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState<
            Evm: ConfigureEvm<
                BlockExecutorFactory: BlockExecutorFactory<EvmFactory: EvmFactory<Tx = TxEnv>>,
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
            >,
            TransactionCompat: TransactionCompat<TxEnv = TxEnvFor<Self::Evm>>,
            Error: FromEvmError<Self::Evm>
                       + From<<Self::TransactionCompat as TransactionCompat>::Error>
                       + From<ProviderError>,
        > + SpawnBlocking,
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
