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

use eyre::Result;
use thiserror::Error;

use futures_util::{stream::FuturesUnordered, Future, Stream, StreamExt};
use reqwest::{Error, StatusCode};
use reth::{
    primitives::{BlobTransaction, B256},
    providers::CanonStateNotification,
    transaction_pool::{BlobStoreError, TransactionPoolExt},
};
use alloy_rpc_types_beacon::sidecar::{BeaconBlobBundle,SidecarIterator};

#[tokio::main]
async fn main() -> Result<()> {
    Ok(())
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, clap::Parser)]
struct BeaconSidecarConfig {
    /// Beacon Node http server address
    #[arg(long = "cl.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub cl_addr: IpAddr,
    /// Beacon Node http server port to listen on
    #[arg(long = "cl.port", default_value_t = 5052)]
    pub cl_port: u16,
}

impl BeaconSidecarConfig {
    pub fn http_base_url(&self) -> String {
        format!("http://{}:{}", self.cl_addr, self.cl_port)
    }

    pub fn sidecar_url(&self, block_root: B256) -> String {
        format!("{}/eth/v1/beacon/blob_sidecars/{}", self.http_base_url(), block_root)
    }
}

// SideCarError Handles Errors from both EL and CL
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
    beacon_config: BeaconSidecarConfig,
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

    /// Attempt to pull the next BlobTransaction from the stream.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this: &mut MinedSidecarStream<St, P> = self.get_mut();

        if let Some(mined_sidecar) = this.queued_actions.pop_front() {
            return Poll::Ready(Some(Ok(mined_sidecar)));
        }

        // Request locally first, otherwise request from CL
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

            while let Poll::Ready(Some(notification)) = this.events.poll_next_unpin(cx) {
                {
                    let mut all_blobs_available = true;
                    let mut actions_to_queue: Vec<BlobTransaction> = Vec::new();

                    // Collect only the EIP4844 transcations.
                    let txs: Vec<_> = notification
                        .tip()
                        .transactions()
                        .filter(|tx: &&reth::primitives::TransactionSigned| tx.is_eip4844())
                        .map(|tx| (tx.clone(), tx.blob_versioned_hashes().unwrap().len()))
                        .collect();

                    if txs.is_empty() {
                        continue;
                    }

                    // Retrieves all blobs in the order in which we requested them
                    match this
                        .pool
                        .get_all_blobs_exact(txs.iter().map(|(tx, _)| tx.hash()).collect())
                    {
                        Ok(blobs) => {
                            // Match the corresponding BlobTransaction with its Transcation
                            for ((tx, _), sidecar) in txs.iter().zip(blobs.iter()) {
                                actions_to_queue.push(BlobTransaction::try_from_signed(tx.clone(), sidecar.clone()).expect(
                                    "should not fail to convert blob tx if it is already eip4844",
                                ));
                            }
                        }
                        Err(_err) => {
                            // If a single BlobTransaction is missing we skip the queue.
                            all_blobs_available = false;
                        }
                    };

                    if all_blobs_available {
                        this.queued_actions.extend(actions_to_queue);
                    } else {
                        let client_clone = this.client.clone();

                        let block_root = notification.tip().block.hash();

                        let sidecar_url = this.beacon_config.sidecar_url(block_root);

                        // Query the Beacon Layer for missing BlobTransactions
                        let query = Box::pin(async move {
                            let response = match client_clone
                                .get(sidecar_url)
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

                            let mut blobs_bundle: BeaconBlobBundle =
                                match serde_json::from_slice(&bytes) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        return Err(SideCarError::DeserializationError(
                                            e.to_string(),
                                        ))
                                    }
                                };

                            let sidecar_iterator = SidecarIterator::new(blobs_bundle);

                            let sidecars: Vec<BlobTransaction> = txs.iter().map(|(tx, blob_len)| {
                                if let Some(sidecar) = sidecar_iterator.next_sidecar(*blob_len) {
                
                                    BlobTransaction::try_from_signed(
                                        tx.clone(),
                                        sidecar,
                                    ).expect("should not fail to convert blob tx if it is already eip4844")
                                } else {
                                    panic!("Expected to find a matching sidecar for every transaction");
                                }
                            }).collect();


                            Ok(sidecars)
                        });

                        this.pending_requests.push(query);
                    }
                }
            }
        }
    }
}