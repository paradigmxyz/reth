//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use std::sync::Arc;

use crate::{EthApiTypes, RpcNodeCoreExt, RpcReceipt};
use alloy_consensus::{transaction::TransactionMeta, TxReceipt};
use futures::Future;
use reth_primitives_traits::{Recovered, RecoveredBlock};
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcConvert};
use reth_rpc_eth_types::utils::calculate_gas_used_and_next_log_index;
use reth_storage_api::{ProviderBlock, ProviderReceipt, ProviderTx};

/// Assembles transaction receipt data w.r.t to network.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` receipts RPC methods.
pub trait LoadReceipt:
    EthApiTypes<RpcConvert: RpcConvert<Primitives = Self::Primitives>> + RpcNodeCoreExt + Send + Sync
{
    /// Helper method for `eth_getBlockReceipts` and `eth_getTransactionReceipt`.
    ///
    /// If `all_receipts` is `Some`, skips the cache lookup for receipts entirely.
    fn build_transaction_receipt(
        &self,
        tx: Recovered<ProviderTx<Self::Provider>>,
        meta: TransactionMeta,
        receipt: ProviderReceipt<Self::Provider>,
        all_receipts: Arc<Vec<ProviderReceipt<Self::Provider>>>,
        block: Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>,
    ) -> impl Future<Output = Result<RpcReceipt<Self::NetworkTypes>, Self::Error>> + Send {
        async move {
            let (gas_used, next_log_index) =
                calculate_gas_used_and_next_log_index(meta.index, &all_receipts);

            Ok(self
                .converter()
                .convert_receipts_with_block(
                    vec![ConvertReceiptInput {
                        tx: tx.as_recovered_ref(),
                        gas_used: receipt.cumulative_gas_used() - gas_used,
                        receipt,
                        next_log_index,
                        meta,
                    }],
                    block.sealed_block(),
                )?
                .pop()
                .unwrap())
        }
    }
}
