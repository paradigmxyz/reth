//! Support for building a pending block with transactions from local view of mempool.

use crate::EthApi;
use alloy_consensus::BlockHeader;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_evm::{ConfigureEvm, NextBlockEnvAttributes};
use reth_node_api::NodePrimitives;
use reth_primitives_traits::SealedHeader;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    types::RpcTypes,
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::PendingBlock;
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, ProviderBlock, ProviderHeader, ProviderReceipt, ProviderTx,
    StateProviderFactory,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm_primitives::B256;

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking<
            NetworkTypes: RpcTypes<
                Header = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>,
            >,
            Error: FromEvmError<Self::Evm>,
            RpcConvert: RpcConvert<Network = Self::NetworkTypes>,
        > + RpcNodeCore<
            Provider: BlockReaderIdExt<Receipt = Provider::Receipt, Block = Provider::Block>
                          + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                          + StateProviderFactory,
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Evm: ConfigureEvm<
                Primitives = <Self as RpcNodeCore>::Primitives,
                NextBlockEnvCtx: From<NextBlockEnvAttributes>,
            >,
            Primitives: NodePrimitives<
                BlockHeader = ProviderHeader<Self::Provider>,
                SignedTx = ProviderTx<Self::Provider>,
                Receipt = ProviderReceipt<Self::Provider>,
                Block = ProviderBlock<Self::Provider>,
            >,
        >,
    Provider: BlockReader,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>,
    > {
        self.inner.pending_block()
    }

    fn next_env_attributes(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<<Self::Evm as reth_evm::ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(NextBlockEnvAttributes {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root().map(|_| B256::ZERO),
            withdrawals: parent.withdrawals_root().map(|_| Default::default()),
        }
        .into())
    }
}
