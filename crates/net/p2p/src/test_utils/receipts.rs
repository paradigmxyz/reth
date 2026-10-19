use crate::{
    download::DownloadClient,
    error::PeerRequestResult,
    priority::Priority,
    receipts::client::{ReceiptsClient, ReceiptsFut, ReceiptsResponse},
};
use alloy_primitives::B256;
use futures::FutureExt;
use reth_ethereum_primitives::Receipt;
use reth_network_peers::PeerId;
use std::fmt::{Debug, Formatter};
use tokio::sync::oneshot;

/// A test client for fetching receipts
pub struct TestReceiptsClient<F> {
    /// The function that is called on each receipt request.
    pub responder: F,
}

impl<F> Debug for TestReceiptsClient<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestReceiptsClient").finish_non_exhaustive()
    }
}

impl<F: Sync + Send> DownloadClient for TestReceiptsClient<F> {
    fn report_bad_message(&self, _peer_id: PeerId) {}

    fn num_connected_peers(&self) -> usize {
        0
    }
}

impl<F> ReceiptsClient for TestReceiptsClient<F>
where
    F: Fn(Vec<B256>) -> PeerRequestResult<ReceiptsResponse<Receipt>> + Send + Sync,
{
    type Receipt = Receipt;
    type Output = ReceiptsFut;

    fn get_receipts_with_priority(&self, hashes: Vec<B256>, _priority: Priority) -> Self::Output {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send((self.responder)(hashes));
        Box::pin(rx.map(|x| match x {
            Ok(value) => value,
            Err(err) => Err(err.into()),
        }))
    }
}
