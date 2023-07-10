use crate::p2p::{
    bodies::client::BodiesClient,
    download::DownloadClient,
    error::PeerRequestResult,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use parking_lot::Mutex;
use reth_primitives::{
    BlockBody, BlockHashOrNumber, BlockNumHash, Header, HeadersDirection, PeerId, SealedBlock,
    SealedHeader, WithPeerId, H256,
};
use std::{collections::HashMap, sync::Arc};

/// A headers+bodies client implementation that does nothing.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct NoopFullBlockClient;

impl DownloadClient for NoopFullBlockClient {
    fn report_bad_message(&self, _peer_id: PeerId) {}

    fn num_connected_peers(&self) -> usize {
        0
    }
}

impl BodiesClient for NoopFullBlockClient {
    type Output = futures::future::Ready<PeerRequestResult<Vec<BlockBody>>>;

    fn get_block_bodies_with_priority(
        &self,
        _hashes: Vec<H256>,
        _priority: Priority,
    ) -> Self::Output {
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), vec![])))
    }
}

impl HeadersClient for NoopFullBlockClient {
    type Output = futures::future::Ready<PeerRequestResult<Vec<Header>>>;

    fn get_headers_with_priority(
        &self,
        _request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), vec![])))
    }
}

/// A headers+bodies client that stores the headers and bodies in memory, with an artificial soft
/// bodies response limit that is set to 20 by default.
///
/// This full block client can be [Clone]d and shared between multiple tasks.
#[derive(Clone, Debug)]
pub struct TestFullBlockClient {
    headers: Arc<Mutex<HashMap<H256, Header>>>,
    bodies: Arc<Mutex<HashMap<H256, BlockBody>>>,
    // soft response limit, max number of bodies to respond with
    soft_limit: usize,
}

impl Default for TestFullBlockClient {
    fn default() -> Self {
        Self {
            headers: Arc::new(Mutex::new(HashMap::new())),
            bodies: Arc::new(Mutex::new(HashMap::new())),
            soft_limit: 20,
        }
    }
}

impl TestFullBlockClient {
    /// Insert a header and body into the client maps.
    pub fn insert(&self, header: SealedHeader, body: BlockBody) {
        let hash = header.hash();
        let header = header.unseal();
        self.headers.lock().insert(hash, header);
        self.bodies.lock().insert(hash, body);
    }

    /// Set the soft response limit.
    pub fn set_soft_limit(&mut self, limit: usize) {
        self.soft_limit = limit;
    }

    /// Get the block with the highest block number.
    pub fn highest_block(&self) -> Option<SealedBlock> {
        let headers = self.headers.lock();
        let (hash, header) = headers.iter().max_by_key(|(hash, header)| header.number)?;
        let bodies = self.bodies.lock();
        let body = bodies.get(hash)?;
        Some(SealedBlock::new(header.clone().seal(*hash), body.clone()))
    }
}

impl DownloadClient for TestFullBlockClient {
    fn report_bad_message(&self, _peer_id: PeerId) {}

    fn num_connected_peers(&self) -> usize {
        1
    }
}

impl HeadersClient for TestFullBlockClient {
    type Output = futures::future::Ready<PeerRequestResult<Vec<Header>>>;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        let headers = self.headers.lock();
        let mut block: BlockHashOrNumber = match request.start {
            BlockHashOrNumber::Hash(hash) => headers.get(&hash).cloned(),
            BlockHashOrNumber::Number(num) => headers.values().find(|h| h.number == num).cloned(),
        }
        .map(|h| h.number.into())
        .unwrap();

        let mut resp = Vec::new();

        for _ in 0..request.limit {
            // fetch from storage
            if let Some((_, header)) = headers.iter().find(|(hash, header)| {
                BlockNumHash::new(header.number, **hash).matches_block_or_num(&block)
            }) {
                match request.direction {
                    HeadersDirection::Falling => block = header.parent_hash.into(),
                    HeadersDirection::Rising => {
                        let next = header.number + 1;
                        block = next.into()
                    }
                }
                resp.push(header.clone());
            } else {
                break
            }
        }
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), resp)))
    }
}

impl BodiesClient for TestFullBlockClient {
    type Output = futures::future::Ready<PeerRequestResult<Vec<BlockBody>>>;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        _priority: Priority,
    ) -> Self::Output {
        let bodies = self.bodies.lock();
        let mut all_bodies = Vec::new();
        for hash in hashes {
            if let Some(body) = bodies.get(&hash) {
                all_bodies.push(body.clone());
            }

            if all_bodies.len() == self.soft_limit {
                break
            }
        }
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), all_bodies)))
    }
}
