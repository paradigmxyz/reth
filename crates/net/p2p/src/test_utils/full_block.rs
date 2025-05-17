use crate::{
    bodies::client::BodiesClient,
    download::DownloadClient,
    error::PeerRequestResult,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
    BlockClient,
};
use alloy_consensus::Header;
use alloy_eips::{BlockHashOrNumber, BlockNumHash};
use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_eth_wire_types::HeadersDirection;
use reth_ethereum_primitives::{Block, BlockBody};
use reth_network_peers::{PeerId, WithPeerId};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use std::{collections::HashMap, sync::Arc};

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
            headers: Arc::new(Mutex::new(HashMap::default())),
            bodies: Arc::new(Mutex::new(HashMap::default())),
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
    pub const fn set_soft_limit(&mut self, limit: usize) {
        self.soft_limit = limit;
    }

    /// Get the block with the highest block number.
    pub fn highest_block(&self) -> Option<SealedBlock<Block>> {
        self.headers.lock().iter().max_by_key(|(_, header)| header.number).and_then(
            |(hash, header)| {
                self.bodies.lock().get(hash).map(|body| {
                    SealedBlock::from_parts_unchecked(header.clone(), body.clone(), *hash)
                })
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
    type Header = Header;
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
                    BlockNumHash::new(header.number, *hash).matches_block_or_num(&block).then(
                        || {
                            match request.direction {
                                HeadersDirection::Falling => block = header.parent_hash.into(),
                                HeadersDirection::Rising => block = (header.number + 1).into(),
                            }
                            header.clone()
                        },
                    )
                })
            })
            .collect::<Vec<_>>();

        // Returns a future containing the retrieved headers with a random peer ID.
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), resp)))
    }
}

/// Implements the `BodiesClient` trait for the `TestFullBlockClient` struct.
impl BodiesClient for TestFullBlockClient {
    type Body = BlockBody;
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

impl BlockClient for TestFullBlockClient {
    type Block = reth_ethereum_primitives::Block;
}
