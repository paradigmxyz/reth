use std::{collections::VecDeque, pin::Pin};

use eyre::Result;
use futures_util::{stream::FuturesUnordered, Future};

use crate::{BeaconSidecarConfig, SideCarError};
use reth::primitives::{BlobTransaction, B256};
use serde::{Deserialize, Serialize};

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
pub enum BlockEvent {
    Mined(MinedBlob),
    Reorged(ReorgedBlob),
}

/// Futures associated with retrieving blob data from the beacon client
type SidecarsFuture = Pin<Box<dyn Future<Output = Result<Vec<BlockEvent>, SideCarError>> + Send>>;

/// Wrapper struct for CanonStateNotifications
pub struct MinedSidecarStream<St, P> {
    pub events: St,
    pub pool: P,
    pub beacon_config: BeaconSidecarConfig,
    pub client: reqwest::Client,
    pub pending_requests: FuturesUnordered<SidecarsFuture>,
    pub queued_actions: VecDeque<BlockEvent>,
}
