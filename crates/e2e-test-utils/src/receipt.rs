//! Helpers for awaiting the receipts of transactions sent through alloy providers.

use alloy_network::{Network, ReceiptResponse};
use alloy_provider::PendingTransactionBuilder;
use eyre::ensure;
use std::future::Future;

/// Extension trait for transactions sent through an alloy provider.
pub trait PendingTransactionExt<N: Network> {
    /// Waits for the receipt of the transaction and returns it, failing if the transaction
    /// reverted.
    ///
    /// Like [`PendingTransactionBuilder::get_receipt`], this waits until the transaction is
    /// included, so the block containing it must be produced, e.g. by
    /// [`NodeTestContext::advance_block`](crate::node::NodeTestContext::advance_block) or
    /// [dev mining](crate::E2ETestSetupBuilder::with_dev_mining).
    fn successful_receipt(self) -> impl Future<Output = eyre::Result<N::ReceiptResponse>> + Send;
}

impl<N: Network> PendingTransactionExt<N> for PendingTransactionBuilder<N> {
    async fn successful_receipt(self) -> eyre::Result<N::ReceiptResponse> {
        let receipt = self.get_receipt().await?;
        ensure!(
            receipt.status(),
            "transaction {} reverted after using {} gas",
            receipt.transaction_hash(),
            receipt.gas_used()
        );
        Ok(receipt)
    }
}

/// Waits for the receipts of all transactions in order, failing if any of them reverted.
///
/// See [`PendingTransactionExt::successful_receipt`].
pub async fn await_successful_receipts<N: Network>(
    txs: impl IntoIterator<Item = PendingTransactionBuilder<N>>,
) -> eyre::Result<Vec<N::ReceiptResponse>> {
    let mut receipts = Vec::new();
    for tx in txs {
        receipts.push(tx.successful_receipt().await?);
    }
    Ok(receipts)
}
