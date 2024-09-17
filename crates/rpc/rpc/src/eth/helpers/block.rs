//! Contains RPC handler implementations specific to blocks.

use reth_primitives::{BlockId, TransactionMeta};
use reth_provider::{BlockReaderIdExt, HeaderProvider};
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock, LoadReceipt, SpawnBlocking},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthStateCache, ReceiptBuilder};
use reth_rpc_types::AnyTransactionReceipt;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthBlocks for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadBlock,
    Provider: HeaderProvider,
{
    #[inline]
    fn provider(&self) -> impl HeaderProvider {
        self.inner.provider()
    }

    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<AnyTransactionReceipt>>, Self::Error>
    where
        Self: LoadReceipt,
    {
        if let Some((block, receipts)) = self.load_block_and_receipts(block_id).await? {
            let block_number = block.number;
            let base_fee = block.base_fee_per_gas;
            let block_hash = block.hash();
            let excess_blob_gas = block.excess_blob_gas;
            let timestamp = block.timestamp;
            let block = block.unseal();

            let receipts = block
                .body
                .into_iter()
                .zip(receipts.iter())
                .enumerate()
                .map(|(idx, (tx, receipt))| {
                    let meta = TransactionMeta {
                        tx_hash: tx.hash,
                        index: idx as u64,
                        block_hash,
                        block_number,
                        base_fee,
                        excess_blob_gas,
                        timestamp,
                    };

                    ReceiptBuilder::new(&tx, meta, receipt, &receipts)
                        .map(|builder| builder.build())
                        .map_err(Self::Error::from_eth_err)
                })
                .collect::<Result<Vec<_>, Self::Error>>();
            return receipts.map(Some)
        }

        Ok(None)
    }
}

impl<Provider, Pool, Network, EvmConfig> LoadBlock for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadPendingBlock + SpawnBlocking,
    Provider: BlockReaderIdExt,
{
    #[inline]
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}
