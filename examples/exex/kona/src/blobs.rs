//! Blob Provider

use anyhow::Result;
use std::boxed::Box;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use reth_primitives::alloy_primitives::{FixedBytes, B256};
use kona_derive::types::{BlockInfo, Blob, BlobProviderError, IndexedBlobHash};
use async_trait::async_trait;
use kona_derive::traits::BlobProvider;

/// A blob provider that hold blobs in memory.
#[derive(Default, Debug, Clone)]
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
pub struct ExExBlobProvider(Arc<Mutex<InMemoryBlobProvider>>);

impl ExExBlobProvider {
    /// Creates a new [ExExBlobProvider].
    pub fn new(inner: Arc<Mutex<InMemoryBlobProvider>>) -> Self {
        Self(inner)
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
        let err = |block_ref: &BlockInfo| BlobProviderError::Custom(anyhow::anyhow!("Blob not found for block ref: {:?}", block_ref));
        let locked = self.0.lock().map_err(|_| err(block_ref))?;
        let blobs_for_block = locked.blocks_to_blob.get(&block_ref.hash).ok_or_else(|| err(block_ref))?;
        let mut blobs = Vec::new();
        for _blob_hash in blob_hashes {
            for blob in blobs_for_block {
                // TODO: update kona-derive to use the alloy_eips Blob type
                // if blob_hash.hash == blob.hash {
                //     blobs.push(*blob.clone());
                //     break;
                // }
                blobs.push(FixedBytes(**blob));
            }
        }
        Ok(blobs)
    }
}
