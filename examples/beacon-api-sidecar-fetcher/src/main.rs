//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-sidecar-fetcher -- node

use std::{
    collections::VecDeque,
    net::IpAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::{stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};
use reqwest::Error;
use reth::{
    primitives::{BlobTransaction, BlobTransactionSidecar, BlockHash, BlockNumber, Transaction},
    providers::CanonStateNotification,
    transaction_pool::TransactionPoolExt,
};

use serde::{self, Deserialize, Serialize};
use tracing::debug;

//TODO look at PeersManager.
//TODO Figure out pending_requests/queued_actions
//TODO Figure out error handling
//TODO Pass (tx, + blockhash/number + sidecar) alongside value
//TODO Seperate Default CL URL logic/function
//TODO Add Tests.

#[tokio::main]
async fn main() -> eyre::Result<()> {
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinedSidecar {
    tx: Transaction,
    block_hash: BlockHash,
    block_number: BlockNumber,
    sidecar: BlobTransactionSidecar,
}

#[derive(Debug)]
pub struct MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
{
    events: St,
    pool: P,
    client: reqwest::Client,
    pending_requests:
        FuturesUnordered<Pin<Box<dyn Future<Output = Result<reqwest::Response, reqwest::Error>>>>>, /* will contant CL queries. */
    queued_actions: VecDeque<BlobTransactionSidecar>, /* Buffer for
                                                       * ready items */
}

impl<St, P> Stream for MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    //TODO Change to custom type
    //type Item = Result<MinedSidecar, SideCarError<E>>;
    type Item = Result<BlobTransactionSidecar, reqwest::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this: &mut MinedSidecarStream<St, P> = self.get_mut();

        // return any buffered result
        if let Some(blob_sidecar) = this.queued_actions.pop_front() {
            return Poll::Ready(Some(Ok(blob_sidecar)));
        }

        // Check if any pending reqwests are ready.
        // TODO Response/Type for CL Request
        while let Poll::Ready(Some(result)) = this.pending_requests.poll_next_unpin(cx) {
            match result {
                // is this array that we than need to loop through?
                Ok(response) => {
                    if response.status().is_success() {
                        // do something with response.
                        //return Poll::Ready(Some(Ok(blob_sidecar)))
                    }
                }
                Err(e) => debug!(error = %e, "Error processing a pending consensus layer request."),
            }
        }

        // TODO: Add fetching logic here.
        loop {
            match this.events.poll_next_unpin(cx) {
                Poll::Ready(Some(notification)) => {
                    for tx in notification.tip().transactions().filter(|tx| tx.is_eip4844()) {
                        match this.pool.get_blob(tx.hash) {
                            Ok(Some(blob)) => {
                                // If get_blob returns Ok(Some(blob)), the blob exists
                                this.queued_actions.push_back(blob);
                            }
                            Ok(None) => {
                                // TODO Move this to default unless specified ot herwise logic?
                                let client_clone = this.client.clone();
                                let url = format!(
                                    "http://{}/eth/v1/beacon/blob_sidecars/{}",
                                    "your-cl-node.com",
                                    notification.tip().block.hash()
                                );

                                // add logic here to get response?
                                let boxed_future = Box::pin(async move {
                                    client_clone
                                        .get(&url)
                                        .header("Accept", "application/json")
                                        .send()
                                        .await
                                });

                                this.pending_requests.push(boxed_future);
                                //TODO send along with MinedSideCar
                                // Return tx with block/etc/with sidecar
                            }

                            // change this
                            Err(e) => {
                                // We store error for this in queued actions?
                                // Lookup error handling
                                debug!(
                                    %e,
                                );
                            }
                        }
                    }
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => continue,
            }
        }
    }
}
