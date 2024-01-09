//! This includes download client implementations for auto sealing miners.

use crate::Storage;
use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    headers::client::{HeadersClient, HeadersFut, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{
    BlockBody, BlockHashOrNumber, Header, HeadersDirection, PeerId, WithPeerId, B256,
};
use std::fmt::Debug;
use tracing::{trace, warn};

/// A download client that polls the miner for transactions and assembles blocks to be returned in
/// the download process.
///
/// When polled, the miner will assemble blocks when miners produce ready transactions and store the
/// blocks in memory.
#[derive(Debug, Clone)]
pub struct AutoSealClient {
    storage: Storage,
}

impl AutoSealClient {
    pub(crate) fn new(storage: Storage) -> Self {
        Self { storage }
    }

    async fn fetch_headers(&self, request: HeadersRequest) -> Vec<Header> {
        trace!(target: "consensus::auto", ?request, "received headers request");

        let storage = self.storage.read().await;
        let HeadersRequest { start, limit, direction } = request;
        let mut headers = Vec::new();

        let mut block: BlockHashOrNumber = match start {
            BlockHashOrNumber::Hash(start) => start.into(),
            BlockHashOrNumber::Number(num) => {
                if let Some(hash) = storage.block_hash(num) {
                    hash.into()
                } else {
                    warn!(target: "consensus::auto", num, "no matching block found");
                    return headers
                }
            }
        };

        for _ in 0..limit {
            // fetch from storage
            if let Some(header) = storage.header_by_hash_or_number(block) {
                match direction {
                    HeadersDirection::Falling => block = header.parent_hash.into(),
                    HeadersDirection::Rising => {
                        let next = header.number + 1;
                        block = next.into()
                    }
                }
                headers.push(header);
            } else {
                break
            }
        }

        trace!(target: "consensus::auto", ?headers, "returning headers");

        headers
    }

    async fn fetch_bodies(&self, hashes: Vec<B256>) -> Vec<BlockBody> {
        trace!(target: "consensus::auto", ?hashes, "received bodies request");
        let storage = self.storage.read().await;
        let mut bodies = Vec::new();
        for hash in hashes {
            if let Some(body) = storage.bodies.get(&hash).cloned() {
                bodies.push(body);
            } else {
                break
            }
        }

        trace!(target: "consensus::auto", ?bodies, "returning bodies");

        bodies
    }
}

impl HeadersClient for AutoSealClient {
    type Output = HeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        let this = self.clone();
        Box::pin(async move {
            let headers = this.fetch_headers(request).await;
            Ok(WithPeerId::new(PeerId::random(), headers))
        })
    }
}

impl BodiesClient for AutoSealClient {
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
    ) -> Self::Output {
        let this = self.clone();
        Box::pin(async move {
            let bodies = this.fetch_bodies(hashes).await;
            Ok(WithPeerId::new(PeerId::random(), bodies))
        })
    }
}

impl DownloadClient for AutoSealClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        warn!("Reported a bad message on a miner, we should never produce bad blocks");
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        // no such thing as connected peers when we are mining ourselves
        1
    }
}
