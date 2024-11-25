//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use futures::Future;
use reth_primitives::{Receipt, TransactionMeta};
use reth_provider::TransactionsProvider;

use crate::{EthApiTypes, RpcNodeCoreExt, RpcReceipt};

/// Assembles transaction receipt data w.r.t to network.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` receipts RPC methods.
pub trait LoadReceipt:
    EthApiTypes + RpcNodeCoreExt<Provider: TransactionsProvider> + Send + Sync
{
    /// Helper method for `eth_getBlockReceipts` and `eth_getTransactionReceipt`.
    fn build_transaction_receipt(
        &self,
        tx: <Self::Provider as TransactionsProvider>::Transaction,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> impl Future<Output = Result<RpcReceipt<Self::NetworkTypes>, Self::Error>> + Send;
}
