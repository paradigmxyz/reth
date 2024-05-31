//! Database access for `eth_` block RPC methods. Loads block and receipt data w.r.t. network.

use std::sync::Arc;

use futures::Future;
use reth_primitives::{BlockId, Receipt, SealedBlock, TransactionMeta};
use reth_provider::{BlockIdReader, BlockReader, BlockReaderIdExt};
use reth_rpc_types::AnyTransactionReceipt;

use crate::eth::{
    api::{BuildReceipt, ReceiptBuilder},
    error::EthResult,
};

/// Commonly used block related functions for the [`EthApiServer`](crate::EthApi) trait in the
/// `eth_` namespace.
pub trait EthBlocks {
    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> &impl BlockReaderIdExt;

    /// Helper function for `eth_getBlockReceipts`.
    ///
    /// Returns all transaction receipts in block, or `None` if block wasn't found.
    fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = EthResult<Option<Vec<AnyTransactionReceipt>>>>
    where
        Self: BuildReceipt,
    {
        async move {
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
                    })
                    .collect::<EthResult<Vec<_>>>();
                return receipts.map(Some)
            }

            Ok(None)
        }
    }

    /// Helper method that loads a bock and all its receipts.
    fn load_block_and_receipts(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = EthResult<Option<(SealedBlock, Arc<Vec<Receipt>>)>>>
    where
        Self: BuildReceipt,
    {
        async move {
            if block_id.is_pending() {
                return Ok(self
                    .provider()
                    .pending_block_and_receipts()?
                    .map(|(sb, receipts)| (sb, Arc::new(receipts))))
            }

            if let Some(block_hash) = self.provider().block_hash_for_id(block_id)? {
                return Ok(self.cache().get_block_and_receipts(block_hash).await?)
            }

            Ok(None)
        }
    }
}
