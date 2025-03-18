//! Support for building a pending block with transactions from local view of mempool.

use alloy_consensus::BlockHeader;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::{ConfigureEvm, NextBlockEnvAttributes};
use reth_node_api::NodePrimitives;
use reth_primitives_traits::SealedHeader;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, ProviderBlock, ProviderHeader,
    ProviderReceipt, ProviderTx, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    types::RpcTypes,
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::PendingBlock;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm_primitives::B256;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking<
            NetworkTypes: RpcTypes<Header = alloy_rpc_types_eth::Header>,
            Error: FromEvmError<Self::Evm>,
        > + RpcNodeCore<
            Provider: BlockReaderIdExt<
                Transaction = reth_ethereum_primitives::TransactionSigned,
                Block = reth_ethereum_primitives::Block,
                Receipt = reth_ethereum_primitives::Receipt,
                Header = alloy_consensus::Header,
            > + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                          + StateProviderFactory,
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Evm: ConfigureEvm<
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                    Receipt = ProviderReceipt<Self::Provider>,
                    Block = ProviderBlock<Self::Provider>,
                >,
                NextBlockEnvCtx = NextBlockEnvAttributes,
            >,
        >,
    Provider: BlockReader<
        Block = reth_ethereum_primitives::Block,
        Receipt = reth_ethereum_primitives::Receipt,
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

    fn next_env_attributes(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<<Self::Evm as reth_evm::ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(NextBlockEnvAttributes {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root(),
            withdrawals: None,
        })
    }
}
