use std::pin::Pin;

use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use alloy_consensus::TxReceipt;
use alloy_primitives::B256;
use futures::Future;
use reth_ethereum_primitives::Receipt;

/// The receipts future type
pub type ReceiptsFut<R = Receipt> =
    Pin<Box<dyn Future<Output = PeerRequestResult<ReceiptsResponse<R>>> + Send + Sync>>;

/// Response from a receipts request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptsResponse<R> {
    /// Receipts grouped by block, in the same order as the requested hashes.
    pub receipts: Vec<Vec<R>>,
    /// When `true`, the **last** block in [`receipts`](Self::receipts) was
    /// truncated by the remote peer (eth/70 `Receipts70.last_block_incomplete`).
    ///
    /// A follow-up `GetReceipts` request for the same block with
    /// `first_block_receipt_index` set to the number of receipts already
    /// received is needed to obtain the remaining receipts.
    pub last_block_incomplete: bool,
}

impl<R> ReceiptsResponse<R> {
    /// Creates a complete (non-truncated) response.
    pub fn new(receipts: Vec<Vec<R>>) -> Self {
        Self { receipts, last_block_incomplete: false }
    }
}

/// A client capable of downloading block receipts from peers.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait ReceiptsClient: DownloadClient {
    /// The receipt type this client fetches.
    type Receipt: TxReceipt;

    /// The output of the request future for querying block receipts.
    type Output: Future<Output = PeerRequestResult<ReceiptsResponse<Self::Receipt>>>
        + Sync
        + Send
        + Unpin;

    /// Fetches the receipts for the requested block hashes.
    fn get_receipts(&self, hashes: Vec<B256>) -> Self::Output {
        self.get_receipts_with_priority(hashes, Priority::Normal)
    }

    /// Fetches the receipts for the requested block hashes with priority.
    fn get_receipts_with_priority(&self, hashes: Vec<B256>, priority: Priority) -> Self::Output;
}
