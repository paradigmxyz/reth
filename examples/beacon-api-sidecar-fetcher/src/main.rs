//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-sidecar-fetcher -- node

use std::{
    collections::VecDeque,
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    task::{Context, Poll},
};

use clap::Parser;
use thiserror::Error;

use futures_util::{stream::FuturesUnordered, Future, Stream, StreamExt};
use reqwest::{Error, StatusCode};
use reth::{
    primitives::BlobTransaction,
    providers::CanonStateNotification,
    rpc::types::engine::BlobsBundleV1,
    transaction_pool::{BlobStoreError, TransactionPoolExt},
};

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
    TransactionPoolError(BlobStoreError),

    #[error("400: {0}")]
    InvalidBlockID(String),

    #[error("404: {0}")]
    BlockNotFound(String),

    #[error("500: {0}")]
    InternalError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Data parsing error: {0}")]
    DeserializationError(String),

    #[error("{0} Error: {1}")]
    UnknownError(u16, String),
}
pub struct MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
{
    events: St,
    pool: P,
    client: reqwest::Client,
    pending_requests:
        FuturesUnordered<Pin<Box<dyn Future<Output = Result<Vec<BlobTransaction>, SideCarError>>>>>, /* TODO make vec */
    queued_actions: VecDeque<BlobTransaction>, /* Buffer for
                                                * ready items */
}

impl<St, P> Stream for MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    type Item = Result<BlobTransaction, SideCarError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this: &mut MinedSidecarStream<St, P> = self.get_mut();
        if let Some(mined_sidecar) = this.queued_actions.pop_front() {
            return Poll::Ready(Some(Ok(mined_sidecar)));
        }

        // return any buffered result
        loop {
            // Check if any pending reqwests are ready and append to buffer
            while let Poll::Ready(Some(pending_result)) = this.pending_requests.poll_next_unpin(cx)
            {
                match pending_result {
                    Ok(mined_sidecars) => {
                        for sidecar in mined_sidecars {
                            this.queued_actions.push_back(sidecar);
                        }
                    }
                    Err(err) => return Poll::Ready(Some(Err(err))),
                }
            }

            // todo: update error for transcations
            // todo: CL connectivty logic
            while let Poll::Ready(Some(notification)) = this.events.poll_next_unpin(cx) {
                {
                    let notification_clone = notification.clone();
                    let mut all_blobs_available = true;
                    let mut actions_to_queue: Vec<BlobTransaction> = Vec::new();

                    let txs: Vec<_> = notification
                        .tip()
                        .transactions()
                        .filter(|tx| tx.is_eip4844())
                        .map(|tx| (tx.clone(), tx.blob_versioned_hashes().unwrap().len()))
                        .collect();

                    //returns
                    match this
                        .pool
                        .get_all_blobs_exact(txs.iter().map(|(tx, _)| tx.hash()).collect())
                    {
                        Ok(blobs) => {
                            for ((tx, _), blob) in txs.iter().zip(blobs.iter()) {
                                match BlobTransaction::try_from_signed(tx.clone(), blob.clone()) {
                                    Ok(blob_transaction) => actions_to_queue.push(blob_transaction),
                                    Err(err) => {
                                        all_blobs_available = false;
                                    }
                                }
                            }
                        }
                        Err(_err) => {
                            all_blobs_available = false;
                        }
                    };

                    if all_blobs_available {
                        this.queued_actions.extend(actions_to_queue);
                    } else {
                        let client_clone = this.client.clone();

                        let url = format!(
                            "http://{}/eth/v1/beacon/blob_sidecars/{}",
                            "your-cl-node.com",
                            notification_clone.tip().block.hash()
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
                                    StatusCode::BAD_REQUEST => Err(SideCarError::InvalidBlockID(
                                        "Invalid request to server.".to_string(),
                                    )),
                                    StatusCode::NOT_FOUND => Err(SideCarError::BlockNotFound(
                                        "Requested block not found.".to_string(),
                                    )),
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
                                Err(e) => return Err(SideCarError::NetworkError(e.to_string())),
                            };

                            // this is all the sidecars for a blob
                            let mut blobs_bundle: BlobsBundleV1 =
                                match serde_json::from_slice(&bytes) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        return Err(SideCarError::DeserializationError(
                                            e.to_string(),
                                        ))
                                    }
                                };

                            let sidecars: Vec<BlobTransaction> = txs
                                .iter()
                                .map(|(tx, blob_len)| {
                                    let sidecar = blobs_bundle.pop_sidecar(*blob_len);
                                    match BlobTransaction::try_from_signed(
                                        tx.clone(),
                                        sidecar.into(),
                                    ) {
                                        Ok(blob_transaction) => blob_transaction,
                                        Err((transaction, sidecar)) => todo!(),
                                    }
                                })
                                .collect();

                            // TODO Logic to figure out sidecar here.
                            Ok(sidecars)
                        });

                        this.pending_requests.push(query);
                    }
                }
            }
        }
    }
}
