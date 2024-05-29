//! Blob Provider

use anyhow::Result;
use async_trait::async_trait;
use kona_derive::{
    online::{OnlineBeaconClient, OnlineBlobProvider, SimpleSlotDerivation},
    traits::BlobProvider,
    types::{alloy_primitives::B256, Blob, BlobProviderError, BlockInfo, IndexedBlobHash},
};
use std::{
    boxed::Box,
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Fallback online blob provider.
pub type OnlineBlobFallback = OnlineBlobProvider<OnlineBeaconClient, SimpleSlotDerivation>;

/// A blob provider that hold blobs in memory.
#[derive(Debug, Clone)]
pub struct InMemoryBlobProvider {
    /// Maps block hashes to blobs.
    blocks_to_blob: HashMap<B256, Vec<Blob>>,
}

impl InMemoryBlobProvider {
    /// Creates a new [InMemoryBlobProvider].
    pub fn new() -> Self {
        Self { blocks_to_blob: HashMap::new() }
    }

    /// Inserts multiple blobs into the provider.
    pub fn insert_blobs(&mut self, block_hash: B256, blobs: Vec<Blob>) {
        self.blocks_to_blob.entry(block_hash).or_default().extend(blobs);
    }
}

/// [BlobProvider] for the [kona_derive::DerivationPipeline].
#[derive(Debug, Clone)]
pub struct ExExBlobProvider(
    Arc<Mutex<InMemoryBlobProvider>>,
    /// Fallback online blob provider.
    /// This is used primarily during sync when archived blobs
    /// aren't provided by reth since they'll be too old.
    OnlineBlobFallback,
);

impl ExExBlobProvider {
    /// Creates a new [ExExBlobProvider].
    pub fn new(inner: Arc<Mutex<InMemoryBlobProvider>>, fallback: OnlineBlobFallback) -> Self {
        Self(inner, fallback)
    }

    /// Attempts to fetch blobs using the inner blob store.
    async fn inner_blob_load(
        &mut self,
        block_ref: &BlockInfo,
        hashes: &[IndexedBlobHash],
    ) -> anyhow::Result<Vec<Blob>> {
        let err = |block_ref: &BlockInfo| {
            anyhow::anyhow!("Blob not found for block ref: {:?}", block_ref)
        };
        let locked = self.0.lock().map_err(|_| anyhow::anyhow!("Failed to lock inner provider"))?;
        let blobs_for_block =
            locked.blocks_to_blob.get(&block_ref.hash).ok_or_else(|| err(block_ref))?;
        let mut blobs = Vec::new();
        for _blob_hash in hashes {
            for blob in blobs_for_block {
                blobs.push(*blob);
            }
        }
        Ok(blobs)
    }
}

#[async_trait]
impl BlobProvider for ExExBlobProvider {
    /// Fetches blobs for a given block ref and the blob hashes.
    async fn get_blobs(
        &mut self,
        block_ref: &BlockInfo,
        blob_hashes: &[IndexedBlobHash],
    ) -> Result<Vec<Blob>, BlobProviderError> {
        if let Ok(b) = self.inner_blob_load(block_ref, blob_hashes).await {
            return Ok(b);
        }
        tracing::warn!(target: "blob-provider", "Blob provider falling back to online provider");
        self.1.get_blobs(block_ref, blob_hashes).await
    }
}
