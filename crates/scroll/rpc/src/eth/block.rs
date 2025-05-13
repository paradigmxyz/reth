//! Loads and formats Scroll block RPC response.

use crate::{eth::ScrollNodeCore, ScrollEthApi, ScrollEthApiError, ScrollReceiptBuilder};

use alloy_consensus::BlockHeader;
use alloy_rpc_types_eth::BlockId;
use reth_chainspec::ChainSpecProvider;
use reth_node_api::BlockBody;
use reth_primitives::TransactionMeta;
use reth_primitives_traits::SignedTransaction;
use reth_provider::{BlockReader, HeaderProvider, ProviderTx};
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock, LoadReceipt, SpawnBlocking},
    types::RpcTypes,
    RpcReceipt,
};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_primitives::{ScrollReceipt, ScrollTransactionSigned};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use scroll_alloy_rpc_types::ScrollTransactionReceipt;

impl<N> EthBlocks for ScrollEthApi<N>
where
    Self: LoadBlock<
        Error = ScrollEthApiError,
        NetworkTypes: RpcTypes<Receipt = ScrollTransactionReceipt>,
        Provider: BlockReader<Receipt = ScrollReceipt, Transaction = ScrollTransactionSigned>,
    >,
    N: ScrollNodeCore<Provider: ChainSpecProvider<ChainSpec = ScrollChainSpec> + HeaderProvider>,
{
    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<RpcReceipt<Self::NetworkTypes>>>, Self::Error>
    where
        Self: LoadReceipt,
    {
        if let Some((block, receipts)) = self.load_block_and_receipts(block_id).await? {
            let block_number = block.number();
            let base_fee = block.base_fee_per_gas();
            let block_hash = block.hash();
            let excess_blob_gas = block.excess_blob_gas();
            let timestamp = block.timestamp();

            return block
                .body()
                .transactions()
                .iter()
                .zip(receipts.iter())
                .enumerate()
                .map(|(idx, (tx, receipt))| -> Result<_, _> {
                    let meta = TransactionMeta {
                        tx_hash: *tx.tx_hash(),
                        index: idx as u64,
                        block_hash,
                        block_number,
                        base_fee,
                        excess_blob_gas,
                        timestamp,
                    };
                    ScrollReceiptBuilder::new(tx, meta, receipt, &receipts)
                        .map(|builder| builder.build())
                })
                .collect::<Result<Vec<_>, Self::Error>>()
                .map(Some)
        }

        Ok(None)
    }
}

impl<N> LoadBlock for ScrollEthApi<N>
where
    Self: LoadPendingBlock<
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
        > + SpawnBlocking,
    N: ScrollNodeCore,
{
}
