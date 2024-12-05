//! Support for building a pending block with transactions from local view of mempool.

use alloy_consensus::Header;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ProviderBlock,
    ProviderReceipt, ProviderTx, StateProviderFactory,
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
    Self: SpawnBlocking<
            NetworkTypes: alloy_network::Network<HeaderResponse = alloy_rpc_types_eth::Header>,
        > + RpcNodeCore<
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
            Evm: ConfigureEvm<Header = Header, Transaction = ProviderTx<Self::Provider>>,
        >,
    Provider: BlockReader<Block = reth_primitives::Block, Receipt = reth_primitives::Receipt>,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>,
    > {
        self.inner.pending_block()
    }

    fn assemble_block(
        &self,
        _parent_hash: revm_primitives::B256,
        _state_root: revm_primitives::B256,
        _transactions: Vec<ProviderTx<Self::Provider>>,
        _receipts: &[reth_provider::ProviderReceipt<Self::Provider>],
    ) -> reth_provider::ProviderBlock<Self::Provider> {
        todo!()
    }

    fn assemble_receipt(
        &self,
        _tx: &reth_primitives::RecoveredTx<ProviderTx<Self::Provider>>,
        _result: revm_primitives::ExecutionResult,
        _cumulative_gas_used: u64,
    ) -> reth_provider::ProviderReceipt<Self::Provider> {
        todo!()
    }
}
