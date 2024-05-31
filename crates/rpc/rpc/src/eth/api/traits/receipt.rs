//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use futures::Future;
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned};
use reth_rpc_types::AnyTransactionReceipt;

use crate::eth::{
    api::ReceiptBuilder,
    cache::EthStateCache,
    error::{EthApiError, EthResult},
};

/// Assembles transaction receipt data w.r.t to network.
pub trait BuildReceipt {
    /// Returns a handle for reading data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn cache(&self) -> &EthStateCache;

    /// Helper method for `eth_getBlockReceipts` and `eth_getTransactionReceipt`.
    fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> impl Future<Output = EthResult<AnyTransactionReceipt>> + Send
    where
        Self: Send + Sync,
    {
        async move {
            // get all receipts for the block
            let all_receipts = match self.cache().get_receipts(meta.block_hash).await? {
                Some(recpts) => recpts,
                None => return Err(EthApiError::UnknownBlockNumber),
            };

            Ok(ReceiptBuilder::new(&tx, meta, &receipt, &all_receipts)?.build())
        }
    }
}
