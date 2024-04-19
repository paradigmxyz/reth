//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-sidecar-fetcher -- node

use std::{
    collections::VecDeque,
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::{
    future::BoxFuture, stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt,
};
use reqwest::{Error, Request, Response};
use reth::{
    primitives::{
        alloy_primitives::TxHash, BlobTransaction, BlobTransactionSidecar, BlockHash, BlockNumber,
        Transaction, B256,
    },
    providers::CanonStateNotification,
    transaction_pool::TransactionPoolExt,
};

use serde::{self, Deserialize, Serialize};
use tracing::debug;

//TODO Figure out error handling
//TODO Pass (tx, + blockhash/number + sidecar) alongside value
//TODO Seperate Default CL URL logic/function
//TODO Add Tests.

#[tokio::main]
async fn main() -> eyre::Result<()> {
    Ok(())
}

#[derive(Debug, Clone)]
pub struct MinedSidecar {
    tx_hash: TxHash,
    block_hash: BlockHash,
    block_number: BlockNumber,
    sidecar: BlobTransactionSidecar,
}

struct PendingMinedSidecar {
    cl_request: Pin<Box<dyn Future<Output = Result<Response, Error>> + Send>>,
    tx_hash: TxHash,
    block_hash: BlockHash,
    block_number: BlockNumber,
}

#[derive(Debug)]
pub struct MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
{
    events: St,
    pool: P,
    client: reqwest::Client,
    pending_requests: FuturesUnordered<Pin<Box<dyn Future<Output = PendingMinedSidecar>>>>, /* will contant CL queries. */
    queued_actions: VecDeque<MinedSidecar>, /* Buffer for
                                             * ready items */
}

impl<St, P> Stream for MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    //TODO Change to custom type
    //type Item = Result<MinedSidecar, SideCarError<E>>;
    type Item = Result<MinedSidecar, reqwest::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this: &mut MinedSidecarStream<St, P> = self.get_mut();

        // return any buffered result
        if let Some(mined_sidecar) = this.queued_actions.pop_front() {
            return Poll::Ready(Some(Ok(mined_sidecar)));
        }

        // Check if any pending reqwests are ready.
        // TODO Response/Type for CL Request
        while let Poll::Ready(Some(pending_result)) = this.pending_requests.poll_next_unpin(cx) {
            match pending_result {
                Ok(mut pending_sidecar) => {
                    // Now poll the future inside PendingMinedSidecar
                    return match pending_sidecar.cl_request.poll_unpin(cx) {
                        Poll::Ready(Ok(response)) => {
                            if response.status().is_success() {
                                // TODO add logic to decode json to side car
                                Poll::Ready(Some(Ok(MinedSidecar {
                                    tx_hash: pending_sidecar.tx_hash,
                                    block_hash: pending_sidecar.block_hash,
                                    block_number: pending_sidecar.block_number,
                                    sidecar: todo!(),
                                })));
                            } else {
                                // Handle HTTP errors
                                return Poll::Ready(Some(Err(todo!())));
                            }
                        }
                        Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
                        Poll::Pending => Poll::Pending,
                    };
                }
                Err(e) => {
                    // Handle errors in fetching the future itself
                    return Poll::Ready(Some(Err(e)));
                }
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
                                this.queued_actions.push_back(MinedSidecar {
                                    tx_hash: tx.hash,
                                    block_hash: notification.tip().block.hash(),
                                    block_number: notification.tip().block.number,
                                    sidecar: blob,
                                });
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

                                let pending_request = PendingMinedSidecar {
                                    cl_request: Box::pin(async move {
                                        client_clone
                                            .get(&url)
                                            .header("Accept", "application/json")
                                            .send()
                                            .await
                                    }),
                                    tx_hash: tx.hash.clone(),
                                    block_hash: notification.tip().block.hash().clone(),
                                    block_number: notification.tip().block.number,
                                };

                                this.pending_requests.push(pending_request);
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
