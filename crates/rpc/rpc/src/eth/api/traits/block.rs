//! Database access for `eth_` block RPC methods. Loads block and receipt data w.r.t. network.

use std::sync::Arc;

use futures::Future;
use reth_primitives::{BlockId, Receipt, SealedBlock, SealedBlockWithSenders, TransactionMeta};
use reth_provider::{BlockIdReader, BlockReader, BlockReaderIdExt};
use reth_rpc_types::AnyTransactionReceipt;

use crate::eth::{
    api::{BuildReceipt, LoadPendingBlock, ReceiptBuilder, SpawnBlocking},
    cache::EthStateCache,
    error::EthResult,
};

/// Block related functions for the [`EthApiServer`](crate::EthApi) trait in the
/// `eth_` namespace.
pub trait EthBlocks: LoadBlock {
    /// Helper function for `eth_getBlockReceipts`.
    ///
    /// Returns all transaction receipts in block, or `None` if block wasn't found.
    fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = EthResult<Option<Vec<AnyTransactionReceipt>>>> + Send
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
}

/// Loads a block from database.
pub trait LoadBlock: Send + Sync {
    // Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> impl BlockReaderIdExt;

    /// Returns a handle for reading data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn cache(&self) -> &EthStateCache;

    /// Returns the block object for the given block id.
    fn block(
        &self,
        block_id: impl Into<BlockId> + Send,
    ) -> impl Future<Output = EthResult<Option<SealedBlock>>> + Send
    where
        Self: LoadPendingBlock + SpawnBlocking,
    {
        async move {
            self.block_with_senders(block_id)
                .await
                .map(|maybe_block| maybe_block.map(|block| block.block))
        }
    }

    /// Returns the block object for the given block id.
    fn block_with_senders(
        &self,
        block_id: impl Into<BlockId> + Send,
    ) -> impl Future<Output = EthResult<Option<SealedBlockWithSenders>>> + Send
    where
        Self: LoadPendingBlock + SpawnBlocking,
    {
        async move {
            let block_id = block_id.into();

            if block_id.is_pending() {
                // Pending block can be fetched directly without need for caching
                let maybe_pending =
                    LoadPendingBlock::provider(self).pending_block_with_senders()?;
                return if maybe_pending.is_some() {
                    Ok(maybe_pending)
                } else {
                    self.local_pending_block().await
                }
            }

            let block_hash = match LoadPendingBlock::provider(self).block_hash_for_id(block_id)? {
                Some(block_hash) => block_hash,
                None => return Ok(None),
            };

            Ok(self.cache().get_sealed_block_with_senders(block_hash).await?)
        }
    }

    /// Helper method that loads a bock and all its receipts.
    fn load_block_and_receipts(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = EthResult<Option<(SealedBlock, Arc<Vec<Receipt>>)>>> + Send
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
                return Ok(BuildReceipt::cache(self).get_block_and_receipts(block_hash).await?)
            }

            Ok(None)
        }
    }
}
