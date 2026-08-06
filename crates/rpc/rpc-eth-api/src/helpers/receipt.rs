//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use std::sync::Arc;

use crate::{EthApiTypes, RpcNodeCoreExt, RpcReceipt};
use alloy_consensus::{transaction::TransactionMeta, TxReceipt};
use futures::Future;
use reth_primitives_traits::{Recovered, RecoveredBlock};
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcConvert};
use reth_rpc_eth_types::{
    error::FromEthApiError, utils::calculate_gas_used_and_next_log_index, EthApiError,
};
use reth_storage_api::{ProviderBlock, ProviderReceipt, ProviderTx};

/// Assembles transaction receipt data w.r.t to network.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` receipts RPC methods.
pub trait LoadReceipt:
    EthApiTypes<RpcConvert: RpcConvert<Primitives = Self::Primitives>> + RpcNodeCoreExt + Send + Sync
{
    /// Helper method for `eth_getBlockReceipts` and `eth_getTransactionReceipt`.
    ///
    /// If a value is `Some`, skips the corresponding cache lookup entirely.
    fn build_transaction_receipt(
        &self,
        tx: Recovered<ProviderTx<Self::Provider>>,
        meta: TransactionMeta,
        receipt: ProviderReceipt<Self::Provider>,
        all_receipts: Option<Arc<Vec<ProviderReceipt<Self::Provider>>>>,
        block: Option<Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>>,
    ) -> impl Future<Output = Result<RpcReceipt<Self::NetworkTypes>, Self::Error>> + Send {
        async move {
            let hash = meta.block_hash;
            let (block, all_receipts) = match (block, all_receipts) {
                (Some(block), Some(all_receipts)) => (Some(block), all_receipts),
                (Some(block), None) => {
                    let all_receipts = self
                        .cache()
                        .get_receipts(hash)
                        .await
                        .map_err(Self::Error::from_eth_err)?
                        .ok_or(EthApiError::HeaderNotFound(hash.into()))?;
                    (Some(block), all_receipts)
                }
                (None, Some(all_receipts)) => {
                    let block = self
                        .cache()
                        .get_maybe_block(hash)
                        .await
                        .map_err(Self::Error::from_eth_err)?;
                    (block, all_receipts)
                }
                (None, None) => {
                    let (all_receipts, block) = self
                        .cache()
                        .get_receipts_and_maybe_block(hash)
                        .await
                        .map_err(Self::Error::from_eth_err)?
                        .ok_or(EthApiError::HeaderNotFound(hash.into()))?;
                    (block, all_receipts)
                }
            };

            let (gas_used, next_log_index) =
                calculate_gas_used_and_next_log_index(meta.index, &all_receipts);

            let inputs = vec![ConvertReceiptInput {
                tx: tx.as_recovered_ref(),
                gas_used: receipt.cumulative_gas_used() - gas_used,
                receipt,
                next_log_index,
                meta,
            }];
            let mut receipts = match block {
                Some(block) => {
                    self.converter().convert_receipts_with_block(inputs, block.sealed_block())
                }
                None => self.converter().convert_receipts(inputs),
            }?;

            Ok(receipts.pop().unwrap())
        }
    }
}
