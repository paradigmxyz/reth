//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use futures::Future;
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned};
use reth_rpc_types::AnyTransactionReceipt;

use crate::{EthApiError, EthResult, EthStateCache, ReceiptBuilder};

/// Assembles transaction receipt data w.r.t to network.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` receipts RPC methods.
#[auto_impl::auto_impl(&, Arc)]
pub trait LoadReceipt: Send + Sync {
    /// Returns a handle for reading data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn cache(&self) -> &EthStateCache;

    /// Helper method for `eth_getBlockReceipts` and `eth_getTransactionReceipt`.
    #[cfg(not(feature = "optimism"))]
    fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> impl Future<Output = EthResult<AnyTransactionReceipt>> + Send {
        async move {
            // get all receipts for the block
            let all_receipts = match self.cache().get_receipts(meta.block_hash).await? {
                Some(recpts) => recpts,
                None => return Err(EthApiError::UnknownBlockNumber),
            };

            Ok(ReceiptBuilder::new(&tx, meta, &receipt, &all_receipts)?.build())
        }
    }

    /// Helper method for `eth_getBlockReceipts` and `eth_getTransactionReceipt`.
    #[cfg(feature = "optimism")]
    fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> impl Future<Output = EthResult<AnyTransactionReceipt>> {
        async move {
            let (block, receipts) = self
                .cache()
                .get_block_and_receipts(meta.block_hash)
                .await?
                .ok_or(EthApiError::UnknownBlockNumber)?;

            let block = block.unseal();
            let l1_block_info = reth_evm_optimism::extract_l1_info(&block).ok();
            let optimism_tx_meta = self.build_op_tx_meta(&tx, l1_block_info, block.timestamp)?;

            let resp_builder = ReceiptBuilder::new(&tx, meta, &receipt, &receipts)?;
            let resp_builder = op_receipt_fields(resp_builder, &tx, &receipt, optimism_tx_meta);

            Ok(resp_builder.build())
        }
    }
}
