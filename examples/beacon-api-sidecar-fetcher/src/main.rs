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

use thiserror::Error;

use futures_util::{
    future::BoxFuture, stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt,
};
use reqwest::{Error, Request, Response, StatusCode};
use reth::{
    network::error,
    primitives::{
        alloy_primitives::TxHash, BlobTransaction, BlobTransactionSidecar, BlockHash, BlockNumber,
        Transaction, B256,
    },
    providers::CanonStateNotification,
    transaction_pool::{BlobStoreError, TransactionPoolExt},
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

//TODO Add nicer errors.
#[derive(Debug, Error)]
pub enum SideCarError {
    #[error("Reqwest encountered an error: {0}")]
    ReqwestError(Error),

    #[error("There was an error grabbing the blob from the tx pool: {0}")]
    TranscanctionPoolError(BlobStoreError),

    #[error("400: {0}")]
    InvalidBlockID(String),

    #[error("404: {0}")]
    BlockNotFound(String),

    #[error("500: {0}")]
    InternalError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Data parsing error: {0}")]
    DataParsingError(String),

    #[error("{0} Error: {1}")]
    UnknownError(u16, String),
}

#[derive(Debug, Clone)]
pub struct MinedSidecar {
    tx_hash: TxHash,
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
        FuturesUnordered<Pin<Box<dyn Future<Output = Result<MinedSidecar, SideCarError>>>>>, /* will contant CL queries. */
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
    type Item = Result<MinedSidecar, SideCarError>;

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
                Ok(mined_sidecar) => return Poll::Ready(Some(Ok(mined_sidecar))),
                Err(err) => return Poll::Ready(Some(Err(err))),
            }
        }

        // TODO: Add fetching logic here.
        loop {
            match this.events.poll_next_unpin(cx) {
                Poll::Ready(Some(notification)) => {
                    for tx in notification.tip().transactions().filter(|tx| tx.is_eip4844()) {
                        match this.pool.get_blob(tx.hash) {
                            Ok(Some(blob)) => {
                                this.queued_actions.push_back(MinedSidecar {
                                    tx_hash: tx.hash,
                                    block_hash: notification.tip().block.hash(),
                                    block_number: notification.tip().block.number,
                                    sidecar: blob,
                                });
                            }
                            Ok(None) => {
                                // TODO Change this
                                let client_clone = this.client.clone();
                                let url = format!(
                                    "http://{}/eth/v1/beacon/blob_sidecars/{}",
                                    "your-cl-node.com",
                                    notification.tip().block.hash()
                                );

                                let query = Box::pin(async move {
                                    let response = match client_clone
                                        .get(url)
                                        .header("Accept", "application/json")
                                        .send()
                                        .await
                                    {
                                        Ok(response) => response,
                                        Err(err) => return Err(SideCarError::ReqwestError(err)),
                                    };

                                    if !response.status().is_success() {
                                        return match response.status() {
                                            StatusCode::BAD_REQUEST => {
                                                Err(SideCarError::InvalidBlockID(
                                                    "Invalid request to server.".to_string(),
                                                ))
                                            }
                                            StatusCode::NOT_FOUND => {
                                                Err(SideCarError::BlockNotFound(
                                                    "Requested block not found.".to_string(),
                                                ))
                                            }
                                            StatusCode::INTERNAL_SERVER_ERROR => {
                                                Err(SideCarError::InternalError(
                                                    "Server encountered an error.".to_string(),
                                                ))
                                            }
                                            _ => Err(SideCarError::UnknownError(
                                                response.status().as_u16(),
                                                "Unhandled HTTP status.".to_string(),
                                            )),
                                        };
                                    }

                                    let bytes = match response.bytes().await {
                                        Ok(b) => b,
                                        Err(e) => {
                                            return Err(SideCarError::NetworkError(e.to_string()))
                                        }
                                    };

                                    // This should be where we turn bytes into a sidecar
                                    let data_str = match std::str::from_utf8(&bytes) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            return Err(SideCarError::DataParsingError(
                                                e.to_string(),
                                            ))
                                        }
                                    };

                                    // TODO Logic to figure out sidecar here.
                                    Ok(MinedSidecar {
                                        tx_hash: tx.hash,
                                        block_hash: notification.tip().block.hash(),
                                        block_number: notification.tip().block.number,
                                        sidecar: data_str.to_string(),
                                    })
                                });

                                this.pending_requests.push(query);
                            }

                            // change this
                            Err(err) => {
                                return Poll::Ready(Some(Err(SideCarError::TranscanctionPoolError(
                                    err,
                                ))))
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
