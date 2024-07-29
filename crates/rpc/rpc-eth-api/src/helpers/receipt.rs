//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use futures::Future;
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned};
use reth_rpc_eth_types::{EthApiError, EthStateCache, ReceiptBuilder};
use reth_rpc_types::AnyTransactionReceipt;

use crate::{EthApiTypes, FromEthApiError};

/// Assembles transaction receipt data w.r.t to network.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` receipts RPC methods.
pub trait LoadReceipt: EthApiTypes + Send + Sync {
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
    ) -> impl Future<Output = Result<AnyTransactionReceipt, Self::Error>> + Send {
        async move {
            // get all receipts for the block
            let all_receipts = match self
                .cache()
                .get_receipts(meta.block_hash)
                .await
                .map_err(Self::Error::from_eth_err)?
            {
                Some(recpts) => recpts,
                None => return Err(EthApiError::UnknownBlockNumber.into()),
            };

            Ok(ReceiptBuilder::new(&tx, meta, &receipt, &all_receipts)?.build())
        }
    }
}
