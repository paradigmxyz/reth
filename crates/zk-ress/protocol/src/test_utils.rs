//! Miscellaneous test utilities.

use crate::{ExecutionProof, ZkRessProtocolProvider};
use alloy_consensus::Header;
use alloy_primitives::{map::B256HashMap, B256};
use reth_ethereum_primitives::BlockBody;
use reth_storage_errors::provider::ProviderResult;
use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Noop implementation of [`ZkRessProtocolProvider`].
#[derive(Clone, Debug)]
pub struct NoopZkRessProtocolProvider<T> {
    __phantom: PhantomData<T>,
}

impl<T> Default for NoopZkRessProtocolProvider<T> {
    fn default() -> Self {
        Self { __phantom: PhantomData }
    }
}

impl<T: ExecutionProof> ZkRessProtocolProvider for NoopZkRessProtocolProvider<T> {
    type Proof = T;

    fn header(&self, _block_hash: B256) -> ProviderResult<Option<Header>> {
        Ok(None)
    }

    fn block_body(&self, _block_hash: B256) -> ProviderResult<Option<BlockBody>> {
        Ok(None)
    }

    async fn proof(&self, _block_hash: B256) -> ProviderResult<Self::Proof> {
        Ok(T::default())
    }
}

/// Mock implementation of [`ZkRessProtocolProvider`].
#[derive(Clone, Default, Debug)]
pub struct MockRessProtocolProvider<T> {
    headers: Arc<Mutex<B256HashMap<Header>>>,
    block_bodies: Arc<Mutex<B256HashMap<BlockBody>>>,
    proofs: Arc<Mutex<B256HashMap<T>>>,
    proof_delay: Option<Duration>,
}

impl<T> MockRessProtocolProvider<T> {
    /// Configure proof response delay.
    pub const fn with_proof_delay(mut self, delay: Duration) -> Self {
        self.proof_delay = Some(delay);
        self
    }

    /// Insert header.
    pub fn add_header(&self, block_hash: B256, header: Header) {
        self.headers.lock().unwrap().insert(block_hash, header);
    }

    /// Extend headers from iterator.
    pub fn extend_headers(&self, headers: impl IntoIterator<Item = (B256, Header)>) {
        self.headers.lock().unwrap().extend(headers);
    }

    /// Insert block body.
    pub fn add_block_body(&self, block_hash: B256, body: BlockBody) {
        self.block_bodies.lock().unwrap().insert(block_hash, body);
    }

    /// Extend block bodies from iterator.
    pub fn extend_block_bodies(&self, bodies: impl IntoIterator<Item = (B256, BlockBody)>) {
        self.block_bodies.lock().unwrap().extend(bodies);
    }

    /// Insert proof.
    pub fn add_proof(&self, block_hash: B256, proof: T) {
        self.proofs.lock().unwrap().insert(block_hash, proof);
    }

    /// Extend proofs from iterator.
    pub fn extend_proofs(&self, proofs: impl IntoIterator<Item = (B256, T)>) {
        self.proofs.lock().unwrap().extend(proofs);
    }
}

impl<T: ExecutionProof> ZkRessProtocolProvider for MockRessProtocolProvider<T> {
    type Proof = T;

    fn header(&self, block_hash: B256) -> ProviderResult<Option<Header>> {
        Ok(self.headers.lock().unwrap().get(&block_hash).cloned())
    }

    fn block_body(&self, block_hash: B256) -> ProviderResult<Option<BlockBody>> {
        Ok(self.block_bodies.lock().unwrap().get(&block_hash).cloned())
    }

    async fn proof(&self, block_hash: B256) -> ProviderResult<Self::Proof> {
        if let Some(delay) = self.proof_delay {
            tokio::time::sleep(delay).await;
        }
        Ok(self.proofs.lock().unwrap().get(&block_hash).cloned().unwrap_or_default())
    }
}
