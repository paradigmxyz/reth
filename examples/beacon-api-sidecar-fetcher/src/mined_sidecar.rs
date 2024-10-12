use crate::BeaconSidecarConfig;
use alloy_primitives::B256;
use alloy_rpc_types_beacon::sidecar::{BeaconBlobBundle, SidecarIterator};
use eyre::Result;
use futures_util::{stream::FuturesUnordered, Future, Stream, StreamExt};
use reqwest::{Error, StatusCode};
use reth::{
    primitives::{BlobTransaction, SealedBlockWithSenders},
    providers::CanonStateNotification,
    transaction_pool::{BlobStoreError, TransactionPoolExt},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMetadata {
    pub block_hash: B256,
    pub block_number: u64,
    pub gas_used: u64,
}

#[derive(Debug, Clone)]
pub struct MinedBlob {
    pub transaction: BlobTransaction,
    pub block_metadata: BlockMetadata,
}

#[derive(Debug, Clone)]
pub struct ReorgedBlob {
    pub transaction_hash: B256,
    pub block_metadata: BlockMetadata,
}

#[derive(Debug, Clone)]
pub enum BlobTransactionEvent {
    Mined(MinedBlob),
    Reorged(ReorgedBlob),
}

/// SideCarError Handles Errors from both EL and CL
#[derive(Debug, Error)]
pub enum SideCarError {
    #[error("Reqwest encountered an error: {0}")]
    ReqwestError(Error),

    #[error("Failed to fetch transactions from the blobstore: {0}")]
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
/// Futures associated with retrieving blob data from the beacon client
type SidecarsFuture =
    Pin<Box<dyn Future<Output = Result<Vec<BlobTransactionEvent>, SideCarError>> + Send>>;

/// A Stream that processes CanonStateNotifications and retrieves BlobTransactions from the beacon
/// client.
///
/// First checks if the blob sidecar for a given EIP4844 is stored locally, if not attempts to
/// retrieve it from the CL Layer
#[must_use = "streams do nothing unless polled"]
pub struct MinedSidecarStream<St, P> {
    pub events: St,
    pub pool: P,
    pub beacon_config: BeaconSidecarConfig,
    pub client: reqwest::Client,
    pub pending_requests: FuturesUnordered<SidecarsFuture>,
    pub queued_actions: VecDeque<BlobTransactionEvent>,
}

impl<St, P> MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    fn process_block(&mut self, block: &SealedBlockWithSenders) {
        let txs: Vec<_> = block
            .transactions()
            .filter(|tx| tx.is_eip4844())
            .map(|tx| (tx.clone(), tx.blob_versioned_hashes().unwrap().len()))
            .collect();

        let mut all_blobs_available = true;
        let mut actions_to_queue: Vec<BlobTransactionEvent> = Vec::new();

        if txs.is_empty() {
            return
        }

        match self.pool.get_all_blobs_exact(txs.iter().map(|(tx, _)| tx.hash()).collect()) {
            Ok(blobs) => {
                for ((tx, _), sidecar) in txs.iter().zip(blobs.iter()) {
                    let transaction = BlobTransaction::try_from_signed(tx.clone(), sidecar.clone())
                        .expect("should not fail to convert blob tx if it is already eip4844");

                    let block_metadata = BlockMetadata {
                        block_hash: block.hash(),
                        block_number: block.number,
                        gas_used: block.gas_used,
                    };
                    actions_to_queue.push(BlobTransactionEvent::Mined(MinedBlob {
                        transaction,
                        block_metadata,
                    }));
                }
            }
            Err(_err) => {
                all_blobs_available = false;
            }
        };

        // if any blob is missing we must instead query the consensus layer.
        if all_blobs_available {
            self.queued_actions.extend(actions_to_queue);
        } else {
            let client_clone = self.client.clone();
            let block_root = block.hash();
            let block_clone = block.clone();
            let sidecar_url = self.beacon_config.sidecar_url(block_root);
            let query =
                Box::pin(fetch_blobs_for_block(client_clone, sidecar_url, block_clone, txs));
            self.pending_requests.push(query);
        }
    }
}

impl<St, P> Stream for MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    type Item = Result<BlobTransactionEvent, SideCarError>;

    /// Attempt to pull the next BlobTransaction from the stream.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Request locally first, otherwise request from CL
        loop {
            if let Some(mined_sidecar) = this.queued_actions.pop_front() {
                return Poll::Ready(Some(Ok(mined_sidecar)))
            }

            // Check if any pending requests are ready and append to buffer
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
                            for (_, block) in new.blocks().iter() {
                                this.process_block(block);
                            }
                        }
                        CanonStateNotification::Reorg { old, new } => {
                            // handle reorged blocks
                            for (_, block) in old.blocks().iter() {
                                let txs: Vec<BlobTransactionEvent> = block
                                    .transactions()
                                    .filter(|tx: &&reth::primitives::TransactionSigned| {
                                        tx.is_eip4844()
                                    })
                                    .map(|tx| {
                                        let transaction_hash = tx.hash();
                                        let block_metadata = BlockMetadata {
                                            block_hash: new.tip().block.hash(),
                                            block_number: new.tip().block.number,
                                            gas_used: new.tip().block.gas_used,
                                        };
                                        BlobTransactionEvent::Reorged(ReorgedBlob {
                                            transaction_hash,
                                            block_metadata,
                                        })
                                    })
                                    .collect();
                                this.queued_actions.extend(txs);
                            }

                            for (_, block) in new.blocks().iter() {
                                this.process_block(block);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Query the Beacon Layer for missing BlobTransactions
async fn fetch_blobs_for_block(
    client: reqwest::Client,
    url: String,
    block: SealedBlockWithSenders,
    txs: Vec<(reth::primitives::TransactionSigned, usize)>,
) -> Result<Vec<BlobTransactionEvent>, SideCarError> {
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
        }
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

    let sidecars: Vec<BlobTransactionEvent> = txs
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
                BlobTransactionEvent::Mined(MinedBlob { transaction, block_metadata })
            })
        })
        .collect();

    Ok(sidecars)
}
