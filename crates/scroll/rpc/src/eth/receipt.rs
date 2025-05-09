//! Loads and formats Scroll receipt RPC response.

use crate::{ScrollEthApi, ScrollEthApiError};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_primitives::TransactionMeta;
use reth_provider::{ReceiptProvider, TransactionsProvider};
use reth_rpc_eth_api::{helpers::LoadReceipt, FromEthApiError, RpcReceipt};
use reth_rpc_eth_types::{receipt::build_receipt, EthApiError};

use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_primitives::{ScrollReceipt, ScrollTransactionSigned};
use scroll_alloy_consensus::ScrollReceiptEnvelope;
use scroll_alloy_rpc_types::{ScrollTransactionReceipt, ScrollTransactionReceiptFields};

impl<N> LoadReceipt for ScrollEthApi<N>
where
    Self: Send + Sync,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ScrollChainSpec>>,
    Self::Provider: TransactionsProvider<Transaction = ScrollTransactionSigned>
        + ReceiptProvider<Receipt = ScrollReceipt>,
{
    async fn build_transaction_receipt(
        &self,
        tx: ScrollTransactionSigned,
        meta: TransactionMeta,
        receipt: ScrollReceipt,
    ) -> Result<RpcReceipt<Self::NetworkTypes>, Self::Error> {
        let all_receipts = self
            .inner
            .eth_api
            .cache()
            .get_receipts(meta.block_hash)
            .await
            .map_err(Self::Error::from_eth_err)?
            .ok_or(Self::Error::from_eth_err(EthApiError::HeaderNotFound(
                meta.block_hash.into(),
            )))?;

        Ok(ScrollReceiptBuilder::new(&tx, meta, &receipt, &all_receipts)?.build())
    }
}

/// Builds an [`ScrollTransactionReceipt`].
#[derive(Debug)]
pub struct ScrollReceiptBuilder {
    /// Core receipt, has all the fields of an L1 receipt and is the basis for the Scroll receipt.
    pub core_receipt: TransactionReceipt<ScrollReceiptEnvelope<Log>>,
    /// Additional Scroll receipt fields.
    pub scroll_receipt_fields: ScrollTransactionReceiptFields,
}

impl ScrollReceiptBuilder {
    /// Returns a new builder.
    pub fn new(
        transaction: &ScrollTransactionSigned,
        meta: TransactionMeta,
        receipt: &ScrollReceipt,
        all_receipts: &[ScrollReceipt],
    ) -> Result<Self, ScrollEthApiError> {
        let core_receipt =
            build_receipt(transaction, meta, receipt, all_receipts, None, |receipt_with_bloom| {
                match receipt {
                    ScrollReceipt::Legacy(_) => {
                        ScrollReceiptEnvelope::<Log>::Legacy(receipt_with_bloom)
                    }
                    ScrollReceipt::Eip2930(_) => {
                        ScrollReceiptEnvelope::<Log>::Eip2930(receipt_with_bloom)
                    }
                    ScrollReceipt::Eip1559(_) => {
                        ScrollReceiptEnvelope::<Log>::Eip1559(receipt_with_bloom)
                    }
                    ScrollReceipt::Eip7702(_) => {
                        ScrollReceiptEnvelope::<Log>::Eip7702(receipt_with_bloom)
                    }
                    ScrollReceipt::L1Message(_) => {
                        ScrollReceiptEnvelope::<Log>::L1Message(receipt_with_bloom)
                    }
                }
            })?;

        let scroll_receipt_fields =
            ScrollTransactionReceiptFields { l1_fee: Some(receipt.l1_fee().saturating_to()) };

        Ok(Self { core_receipt, scroll_receipt_fields })
    }

    /// Builds [`ScrollTransactionReceipt`] by combing core (l1) receipt fields and additional
    /// Scroll receipt fields.
    pub fn build(self) -> ScrollTransactionReceipt {
        let Self { core_receipt: inner, scroll_receipt_fields } = self;

        let ScrollTransactionReceiptFields { l1_fee, .. } = scroll_receipt_fields;

        ScrollTransactionReceipt { inner, l1_fee }
    }
}
