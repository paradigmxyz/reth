//! Loads a receipt from database. Helper trait for `eth_` block and transaction RPC methods, that
//! loads receipt data w.r.t. network.

use crate::{EthApiTypes, RpcNodeCoreExt, RpcReceipt};
use alloy_consensus::{transaction::TransactionMeta, TxReceipt};
use futures::Future;
use reth_primitives_traits::SignerRecoverable;
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcConvert};
use reth_rpc_eth_types::{error::FromEthApiError, EthApiError};
use reth_storage_api::{ProviderReceipt, ProviderTx};

/// Calculates the gas used and next log index for a transaction at the given index
pub fn calculate_gas_used_and_next_log_index(
    tx_index: u64,
    all_receipts: &[impl TxReceipt],
) -> (u64, usize) {
    let mut gas_used = 0;
    let mut next_log_index = 0;

    if tx_index > 0 {
        for receipt in all_receipts.iter().take(tx_index as usize) {
            gas_used = receipt.cumulative_gas_used();
            next_log_index += receipt.logs().len();
        }
    }

    (gas_used, next_log_index)
}

/// Assembles transaction receipt data w.r.t to network.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` receipts RPC methods.
pub trait LoadReceipt:
    EthApiTypes<
        RpcConvert: RpcConvert<
            Primitives = Self::Primitives,
            Error = Self::Error,
            Network = Self::NetworkTypes,
        >,
        Error: FromEthApiError,
    > + RpcNodeCoreExt
    + Send
    + Sync
{
    /// Helper method for `eth_getBlockReceipts` and `eth_getTransactionReceipt`.
    fn build_transaction_receipt(
        &self,
        tx: ProviderTx<Self::Provider>,
        meta: TransactionMeta,
        receipt: ProviderReceipt<Self::Provider>,
    ) -> impl Future<Output = Result<RpcReceipt<Self::NetworkTypes>, Self::Error>> + Send {
        async move {
            let hash = meta.block_hash;
            // get all receipts for the block
            let all_receipts = self
                .cache()
                .get_receipts(hash)
                .await
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(hash.into()))?;

            let (gas_used, next_log_index) =
                calculate_gas_used_and_next_log_index(meta.index, &all_receipts);

            Ok(self
                .tx_resp_builder()
                .convert_receipts(vec![ConvertReceiptInput {
                    tx: tx
                        .try_into_recovered_unchecked()
                        .map_err(Self::Error::from_eth_err)?
                        .as_recovered_ref(),
                    gas_used: receipt.cumulative_gas_used() - gas_used,
                    receipt,
                    next_log_index,
                    meta,
                }])?
                .pop()
                .unwrap())
        }
    }
}
