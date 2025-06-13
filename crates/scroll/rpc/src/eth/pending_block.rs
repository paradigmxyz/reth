//! Loads Scroll pending block for an RPC response.

use crate::ScrollEthApi;

use alloy_consensus::{BlockHeader, Header};
use reth_chainspec::EthChainSpec;
use reth_evm::ConfigureEvm;
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, ProviderBlock, ProviderHeader, ProviderReceipt,
    ProviderTx, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    types::RpcTypes,
    EthApiTypes, RpcNodeCore,
};
use reth_rpc_eth_types::{error::FromEvmError, PendingBlock};
use reth_scroll_evm::ScrollNextBlockEnvAttributes;
use reth_scroll_primitives::{ScrollBlock, ScrollReceipt, ScrollTransactionSigned};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use scroll_alloy_hardforks::ScrollHardforks;

impl<N> LoadPendingBlock for ScrollEthApi<N>
where
    Self: SpawnBlocking
        + EthApiTypes<
            NetworkTypes: RpcTypes<
                Header = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>,
            >,
            Error: FromEvmError<Self::Evm>,
        >,
    N: RpcNodeCore<
        Provider: BlockReaderIdExt<
            Transaction = ScrollTransactionSigned,
            Block = ScrollBlock,
            Receipt = ScrollReceipt,
            Header = Header,
        > + ChainSpecProvider<ChainSpec: EthChainSpec + ScrollHardforks>
                      + StateProviderFactory,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<N::Provider>>>,
        Evm: ConfigureEvm<
            Primitives = <Self as RpcNodeCore>::Primitives,
            NextBlockEnvCtx = ScrollNextBlockEnvAttributes,
        >,
        Primitives: NodePrimitives<
            BlockHeader = ProviderHeader<Self::Provider>,
            SignedTx = ProviderTx<Self::Provider>,
            Receipt = ProviderReceipt<Self::Provider>,
            Block = ProviderBlock<Self::Provider>,
        >,
    >,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>,
    > {
        self.inner.eth_api.pending_block()
    }

    fn next_env_attributes(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<<Self::Evm as ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(ScrollNextBlockEnvAttributes {
            timestamp: parent.timestamp().saturating_add(3),
            suggested_fee_recipient: parent.beneficiary(),
            gas_limit: parent.gas_limit(),
            base_fee: parent.base_fee_per_gas().unwrap_or_default(),
        })
    }
}
