use super::ScrollNodeCore;
use crate::{ScrollEthApi, ScrollEthApiError};

use alloy_rpc_types_eth::transaction::TransactionRequest;
use reth_evm::{block::BlockExecutorFactory, ConfigureEvm, EvmFactory, TxEnvFor};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{errors::ProviderError, ProviderHeader, ProviderTx};
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall, LoadBlock, LoadState, SpawnBlocking},
    FullEthApiTypes, RpcConvert, RpcTypes,
};
use reth_rpc_eth_types::error::FromEvmError;
use revm::context::TxEnv;
use scroll_alloy_evm::ScrollTransactionIntoTxEnv;

impl<N> EthCall for ScrollEthApi<N>
where
    Self: EstimateCall + LoadBlock + FullEthApiTypes,
    N: ScrollNodeCore,
{
}

impl<N> EstimateCall for ScrollEthApi<N>
where
    Self: Call,
    Self::Error: From<ScrollEthApiError>,
    N: ScrollNodeCore,
{
}

impl<N> Call for ScrollEthApi<N>
where
    Self: LoadState<
            Evm: ConfigureEvm<
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
                BlockExecutorFactory: BlockExecutorFactory<
                    EvmFactory: EvmFactory<Tx = ScrollTransactionIntoTxEnv<TxEnv>>,
                >,
            >,
            RpcConvert: RpcConvert<TxEnv = TxEnvFor<Self::Evm>, Network = Self::NetworkTypes>,
            NetworkTypes: RpcTypes<TransactionRequest: From<TransactionRequest>>,
            Error: FromEvmError<Self::Evm>
                       + From<<Self::RpcConvert as RpcConvert>::Error>
                       + From<ProviderError>,
        > + SpawnBlocking,
    Self::Error: From<ScrollEthApiError>,
    N: ScrollNodeCore,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.eth_api.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.eth_api.max_simulate_blocks()
    }
}
