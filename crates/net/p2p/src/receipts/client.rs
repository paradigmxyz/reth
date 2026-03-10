use std::pin::Pin;

use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use alloy_consensus::TxReceipt;
use alloy_primitives::B256;
use futures::Future;
use reth_eth_wire_types::Receipts70;

/// The receipts future type
pub type ReceiptsFut<R = reth_ethereum_primitives::Receipt> =
    Pin<Box<dyn Future<Output = PeerRequestResult<ReceiptsResponse<R>>> + Send + Sync>>;

/// Response from a receipts request.
///
/// **Note for [`ReceiptsClient`] callers:** the network layer handles eth/70
/// continuation rounds internally, so `last_block_incomplete` is always `false`
/// by the time this response reaches a [`ReceiptsClient`] consumer. The field
/// exists for internal use by the fetcher during multi-round assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptsResponse<R> {
    /// Receipts grouped by block, in the same order as the requested hashes.
    pub receipts: Vec<Vec<R>>,
    /// When `true`, the **last** block in [`receipts`](Self::receipts) was
    /// truncated by the remote peer (eth/70 `Receipts70.last_block_incomplete`).
    ///
    /// This is used internally by the fetcher to drive continuation rounds.
    /// Responses surfaced through [`ReceiptsClient`] always have this set to
    /// `false`.
    pub last_block_incomplete: bool,
}

impl<R> ReceiptsResponse<R> {
    /// Creates a complete (non-truncated) response.
    #[inline]
    pub const fn new(receipts: Vec<Vec<R>>) -> Self {
        Self { receipts, last_block_incomplete: false }
    }
}

impl<R> From<Receipts70<R>> for ReceiptsResponse<R> {
    fn from(r70: Receipts70<R>) -> Self {
        Self { receipts: r70.receipts, last_block_incomplete: r70.last_block_incomplete }
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
