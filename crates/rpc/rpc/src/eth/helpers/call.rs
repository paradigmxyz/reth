//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use alloy_evm::block::BlockExecutorFactory;
use alloy_rpc_types_eth::TransactionRequest;
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

impl<N> EthCall for EthApi<N>
where
    N: RpcNodeCore<Provider: BlockReader>,
    Self: EstimateCall
        + LoadPendingBlock
        + FullEthApiTypes
        + RpcNodeCoreExt<
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Primitives: NodePrimitives<SignedTx = ProviderTx<Self::Provider>>,
            Evm = N::Evm,
        >,
    N::Evm: ConfigureEvm<Primitives = <Self as RpcNodeCore>::Primitives>,
{
}

impl<N> Call for EthApi<N>
where
    N: RpcNodeCore<Provider: BlockReader>,
    Self: LoadState<
            Evm: ConfigureEvm<
                BlockExecutorFactory: BlockExecutorFactory<EvmFactory: EvmFactory<Tx = TxEnv>>,
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
            >,
            RpcConvert: RpcConvert<TxEnv = TxEnvFor<Self::Evm>, Network = Self::NetworkTypes>,
            NetworkTypes: RpcTypes<TransactionRequest: From<TransactionRequest>>,
            Error: FromEvmError<Self::Evm>
                       + From<<Self::RpcConvert as RpcConvert>::Error>
                       + From<ProviderError>,
        > + SpawnBlocking,
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

impl<N> EstimateCall for EthApi<N>
where
    N: RpcNodeCore<Provider: BlockReader>,
    Self: Call,
{
}
