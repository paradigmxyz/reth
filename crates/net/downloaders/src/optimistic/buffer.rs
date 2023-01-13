use lru::LruCache;
use reth_eth_wire::BlockBody;
use reth_primitives::{SealedHeader, H256};
use std::num::NonZeroUsize;

/// The downloader buffer.
#[derive(Debug)]
pub(crate) struct DownloadBuffer {
    // TODO: merge two buffers?
    /// Internal header buffer
    headers: LruCache<H256, SealedHeader>,
    // /// Internal body buffer
    bodies: LruCache<H256, BlockBody>,
}

impl DownloadBuffer {
    /// Create new instance of [DownloadBuffer].
    pub(crate) fn new(size: usize) -> Self {
        let size = NonZeroUsize::new(size).expect("invalid buffer size");
        Self { headers: LruCache::new(size), bodies: LruCache::new(size) }
    }
}

impl DownloadBuffer {
    /// Add headers to the buffer.
    pub(crate) fn extend_headers(&mut self, headers: impl Iterator<Item = SealedHeader>) {
        for header in headers {
            self.headers.put(header.hash(), header);
        }
    }

    /// Add bodies to the buffer.
    pub(crate) fn extend_bodies(&mut self, bodies: impl Iterator<Item = BlockBody>) {
        for _body in bodies {
            // TODO: self.bodies.put(...);
        }
    }

    /// Retrieve header from buffer if it exists
    pub(crate) fn retrieve_header(&mut self, hash: H256) -> Option<SealedHeader> {
        self.headers.get(&hash).cloned()
    }

    /// Retrieve body from buffer if it exists.
    pub(crate) fn retrieve_body(&mut self, hash: H256) -> Option<BlockBody> {
        self.bodies.get(&hash).cloned()
    }

    /// Retrieve headers from the buffer by walking the chain.
    /// Next header is the parent of the previous one if any. The collection is
    /// returned when the parent does not exist or we reached the termination hash.
    pub(crate) fn retrieve_header_chain(
        &mut self,
        tip: H256,
        until: H256,
    ) -> Option<Vec<SealedHeader>> {
        let tip = self.headers.get(&tip)?.clone();
        let mut next = tip.parent_hash;
        let mut chain = vec![tip];
        while let Some(parent) = self.headers.get(&next) {
            next = parent.parent_hash;
            chain.push(parent.clone());

            if parent.hash() == until {
                break
            }
        }
        Some(chain)
    }
}
