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
    SealedHeader, WithPeerId, B256,
};
use std::{collections::HashMap, sync::Arc};

/// A headers+bodies client implementation that does nothing.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct NoopFullBlockClient;

/// Implements the `DownloadClient` trait for the `NoopFullBlockClient` struct.
impl DownloadClient for NoopFullBlockClient {
    /// Reports a bad message received from a peer.
    ///
    /// # Arguments
    ///
    /// * `_peer_id` - Identifier for the peer sending the bad message (unused in this
    ///   implementation).
    fn report_bad_message(&self, _peer_id: PeerId) {}

    /// Retrieves the number of connected peers.
    ///
    /// # Returns
    ///
    /// The number of connected peers, which is always zero in this implementation.
    fn num_connected_peers(&self) -> usize {
        0
    }
}

/// Implements the `BodiesClient` trait for the `NoopFullBlockClient` struct.
impl BodiesClient for NoopFullBlockClient {
    /// Defines the output type of the function.
    type Output = futures::future::Ready<PeerRequestResult<Vec<BlockBody>>>;

    /// Retrieves block bodies based on provided hashes and priority.
    ///
    /// # Arguments
    ///
    /// * `_hashes` - A vector of block hashes (unused in this implementation).
    /// * `_priority` - Priority level for block body retrieval (unused in this implementation).
    ///
    /// # Returns
    ///
    /// A future containing an empty vector of block bodies and a randomly generated `PeerId`.
    fn get_block_bodies_with_priority(
        &self,
        _hashes: Vec<B256>,
        _priority: Priority,
    ) -> Self::Output {
        // Create a future that immediately returns an empty vector of block bodies and a random
        // PeerId.
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), vec![])))
    }
}

impl HeadersClient for NoopFullBlockClient {
    /// The output type representing a future containing a peer request result with a vector of
    /// headers.
    type Output = futures::future::Ready<PeerRequestResult<Vec<Header>>>;

    /// Retrieves headers with a specified priority level.
    ///
    /// This implementation does nothing and returns an empty vector of headers.
    ///
    /// # Arguments
    ///
    /// * `_request` - A request for headers (unused in this implementation).
    /// * `_priority` - The priority level for the headers request (unused in this implementation).
    ///
    /// # Returns
    ///
    /// Always returns a ready future with an empty vector of headers wrapped in a
    /// `PeerRequestResult`.
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
    headers: Arc<Mutex<HashMap<B256, Header>>>,
    bodies: Arc<Mutex<HashMap<B256, BlockBody>>>,
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
        self.headers.lock().insert(hash, header.unseal());
        self.bodies.lock().insert(hash, body);
    }

    /// Set the soft response limit.
    pub fn set_soft_limit(&mut self, limit: usize) {
        self.soft_limit = limit;
    }

    /// Get the block with the highest block number.
    pub fn highest_block(&self) -> Option<SealedBlock> {
        self.headers.lock().iter().max_by_key(|(_, header)| header.number).and_then(
            |(hash, header)| {
                self.bodies
                    .lock()
                    .get(hash)
                    .map(|body| SealedBlock::new(header.clone().seal(*hash), body.clone()))
            },
        )
    }
}

impl DownloadClient for TestFullBlockClient {
    /// Reports a bad message from a specific peer.
    fn report_bad_message(&self, _peer_id: PeerId) {}

    /// Retrieves the number of connected peers.
    ///
    /// Returns the number of connected peers in the test scenario (1).
    fn num_connected_peers(&self) -> usize {
        1
    }
}

/// Implements the `HeadersClient` trait for the `TestFullBlockClient` struct.
impl HeadersClient for TestFullBlockClient {
    /// Specifies the associated output type.
    type Output = futures::future::Ready<PeerRequestResult<Vec<Header>>>;

    /// Retrieves headers with a given priority level.
    ///
    /// # Arguments
    ///
    /// * `request` - A `HeadersRequest` indicating the headers to retrieve.
    /// * `_priority` - A `Priority` level for the request.
    ///
    /// # Returns
    ///
    /// A `Ready` future containing a `PeerRequestResult` with a vector of retrieved headers.
    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        let headers = self.headers.lock();

        // Initializes the block hash or number.
        let mut block: BlockHashOrNumber = match request.start {
            BlockHashOrNumber::Hash(hash) => headers.get(&hash).cloned(),
            BlockHashOrNumber::Number(num) => headers.values().find(|h| h.number == num).cloned(),
        }
        .map(|h| h.number.into())
        .unwrap();

        // Retrieves headers based on the provided limit and request direction.
        let resp = (0..request.limit)
            .filter_map(|_| {
                headers.iter().find_map(|(hash, header)| {
                    // Checks if the header matches the specified block or number.
                    if BlockNumHash::new(header.number, *hash).matches_block_or_num(&block) {
                        match request.direction {
                            HeadersDirection::Falling => block = header.parent_hash.into(),
                            HeadersDirection::Rising => block = (header.number + 1).into(),
                        }
                        Some(header.clone())
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();

        // Returns a future containing the retrieved headers with a random peer ID.
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), resp)))
    }
}

/// Implements the `BodiesClient` trait for the `TestFullBlockClient` struct.
impl BodiesClient for TestFullBlockClient {
    /// Defines the output type of the function.
    type Output = futures::future::Ready<PeerRequestResult<Vec<BlockBody>>>;

    /// Retrieves block bodies corresponding to provided hashes with a given priority.
    ///
    /// # Arguments
    ///
    /// * `hashes` - A vector of block hashes to retrieve bodies for.
    /// * `_priority` - Priority level for block body retrieval (unused in this implementation).
    ///
    /// # Returns
    ///
    /// A future containing the result of the block body retrieval operation.
    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
    ) -> Self::Output {
        // Acquire a lock on the bodies.
        let bodies = self.bodies.lock();

        // Create a future that immediately returns the result of the block body retrieval
        // operation.
        futures::future::ready(Ok(WithPeerId::new(
            PeerId::random(),
            hashes
                .iter()
                .filter_map(|hash| bodies.get(hash).cloned())
                .take(self.soft_limit)
                .collect(),
        )))
    }
}
