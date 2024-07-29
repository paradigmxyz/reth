//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use futures::Future;
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned};
use reth_rpc_eth_types::{EthApiError, EthResult, EthStateCache, ReceiptBuilder};
use reth_rpc_types::AnyTransactionReceipt;

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
}
