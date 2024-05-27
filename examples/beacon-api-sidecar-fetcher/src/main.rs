//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-beacon-sidecar-fetcher --node -- full
//! ```
//!
//! This launches a regular reth instance and subscribes to payload attributes event stream.
//!
//! **NOTE**: This expects that the CL client is running an http server on `localhost:5052` and is
//! configured to emit payload attributes events.
//!
//! See lighthouse beacon Node API: <https://lighthouse-book.sigmaprime.io/api-bn.html#beacon-node-api>

use std::{
    collections::VecDeque,
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    task::{Context, Poll},
};

use alloy_rpc_types_beacon::sidecar::{BeaconBlobBundle, SidecarIterator};
use clap::Parser;
use eyre::Result;
use futures_util::{stream::FuturesUnordered, Stream, StreamExt};
use mined_sidecar::{BlockEvent, BlockMetadata, MinedBlob, MinedSidecarStream, ReorgedBlob};
use reqwest::{Error, StatusCode};
use reth::{
    builder::NodeHandle,
    cli::Cli,
    primitives::{BlobTransaction, SealedBlock, B256},
    providers::{CanonStateNotification, CanonStateSubscriptions},
    transaction_pool::{BlobStoreError, TransactionPoolExt},
};
use reth_node_ethereum::EthereumNode;
use thiserror::Error;
pub mod mined_sidecar;

fn main() {
    Cli::<BeaconSidecarConfig>::parse()
        .run(|builder, args| async move {
            // launch the node
            let NodeHandle { node, node_exit_future } =
                builder.node(EthereumNode::default()).launch().await?;

            let notifications: reth::providers::CanonStateNotificationStream =
                node.provider.canonical_state_stream();

            let pool = node.pool.clone();

            let mut sidecar_stream = MinedSidecarStream {
                events: notifications,
                pool,
                beacon_config: args,
                client: reqwest::Client::new(),
                pending_requests: FuturesUnordered::new(),
                queued_actions: VecDeque::new(),
            };

            while let Some(result) = sidecar_stream.next().await {
                match result {
                    Ok(blob_transaction) => {
                        // Handle successful transaction
                        println!("Processed BlobTransaction: {:?}", blob_transaction);
                    }
                    Err(e) => {
                        // Handle errors specifically
                        eprintln!("Failed to process transaction: {:?}", e);
                    }
                }
            }
            node_exit_future.await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, clap::Parser)]
pub struct BeaconSidecarConfig {
    /// Beacon Node http server address
    #[arg(long = "cl.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub cl_addr: IpAddr,
    /// Beacon Node http server port to listen on
    #[arg(long = "cl.port", default_value_t = 5052)]
    pub cl_port: u16,
}

impl Default for BeaconSidecarConfig {
    /// Default setup for lighthouse client
    fn default() -> Self {
        Self {
            cl_addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), // Equivalent to Ipv4Addr::LOCALHOST
            cl_port: 5052,
        }
    }
}

impl BeaconSidecarConfig {
    /// Returns the http url of the beacon node
    pub fn http_base_url(&self) -> String {
        format!("http://{}:{}", self.cl_addr, self.cl_port)
    }

    /// Returns the URL to the beacon sidecars endpoint (https://ethereum.github.io/beacon-APIs/#/Beacon/getBlobSidecars)
    pub fn sidecar_url(&self, block_root: B256) -> String {
        format!("{}/eth/v1/beacon/blob_sidecars/{}", self.http_base_url(), block_root)
    }
}

/// SideCarError Handles Errors from both EL and CL
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

/// First checks if the Blobtranscation for a given EIP4844 is stored locally, if not attempts to
/// retrieve it from the CL Layer
impl<St, P> Stream for MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    type Item = Result<BlockEvent, SideCarError>;

    /// Attempt to pull the next BlobTransaction from the stream.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Request locally first, otherwise request from CL
        loop {
            if let Some(mined_sidecar) = this.queued_actions.pop_front() {
                return Poll::Ready(Some(Ok(mined_sidecar)));
            }

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
                    match notification {
                        CanonStateNotification::Commit { new } => {
                            let mut all_blobs_available = true;
                            let mut actions_to_queue: Vec<BlockEvent> = Vec::new();

                            // Collect only the EIP4844 transcations.
                            let txs: Vec<_> = new
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
                                        let transaction = BlobTransaction::try_from_signed(tx.clone(), sidecar.clone())
                                            .expect("should not fail to convert blob tx if it is already eip4844");

                                        let block_metadata = BlockMetadata {
                                            block_hash: new.tip().block.hash(),
                                            block_number: new.tip().block.number,
                                            gas_used: new.tip().block.gas_used,
                                        };
                                        actions_to_queue.push(BlockEvent::Mined(MinedBlob {
                                            transaction,
                                            block_metadata,
                                        }));
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
                                let block = new.tip().block.clone();
                                let block_root = new.tip().block.hash();
                                let sidecar_url = this.beacon_config.sidecar_url(block_root);
                                let query = Box::pin(fetch_blobs_from_consensus_layer(
                                    client_clone,
                                    sidecar_url,
                                    block,
                                    txs,
                                ));
                                this.pending_requests.push(query);
                            }
                        }
                        CanonStateNotification::Reorg { old, new } => {
                            let txs: Vec<BlockEvent> = new
                                .tip()
                                .transactions()
                                .filter(|tx: &&reth::primitives::TransactionSigned| tx.is_eip4844())
                                .map(|tx| {
                                    let transaction_hash = tx.hash();
                                    let block_metadata = BlockMetadata {
                                        block_hash: new.tip().block.hash(),
                                        block_number: new.tip().block.number,
                                        gas_used: new.tip().block.gas_used,
                                    };
                                    BlockEvent::Reorged(ReorgedBlob {
                                        transaction_hash,
                                        block_metadata,
                                    })
                                })
                                .collect();

                            this.queued_actions.extend(txs);
                        }
                    }
                }
            }
        }
    }
}

/// Query the Beacon Layer for missing BlobTransactions
async fn fetch_blobs_from_consensus_layer(
    client: reqwest::Client,
    url: String,
    block: SealedBlock,
    txs: Vec<(reth::primitives::TransactionSigned, usize)>,
) -> Result<Vec<BlockEvent>, SideCarError> {
    let response = match client.get(url).header("Accept", "application/json").send().await {
        Ok(response) => response,
        Err(err) => return Err(SideCarError::ReqwestError(err)),
    };

    if !response.status().is_success() {
        return match response.status() {
            StatusCode::BAD_REQUEST => {
                Err(SideCarError::InvalidBlockID("Invalid request to server.".to_string()))
            }
            StatusCode::NOT_FOUND => {
                Err(SideCarError::BlockNotFound("Requested block not found.".to_string()))
            }
            StatusCode::INTERNAL_SERVER_ERROR => {
                Err(SideCarError::InternalError("Server encountered an error.".to_string()))
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

    let blobs_bundle: BeaconBlobBundle = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return Err(SideCarError::DeserializationError(e.to_string())),
    };

    let mut sidecar_iterator = SidecarIterator::new(blobs_bundle);

    let sidecars: Vec<BlockEvent> = txs
        .iter()
        .filter_map(|(tx, blob_len)| {
            sidecar_iterator.next_sidecar(*blob_len).map(|sidecar| {
                let transaction = BlobTransaction::try_from_signed(tx.clone(), sidecar)
                    .expect("should not fail to convert blob tx if it is already eip4844");
                let block_metadata = BlockMetadata {
                    block_hash: block.hash(),
                    block_number: block.number,
                    gas_used: block.gas_used,
                };
                BlockEvent::Mined(MinedBlob { transaction, block_metadata })
            })
        })
        .collect();

    Ok(sidecars)
}
