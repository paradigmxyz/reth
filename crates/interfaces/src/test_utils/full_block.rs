use crate::p2p::{
    bodies::client::BodiesClient,
    download::DownloadClient,
    error::PeerRequestResult,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{BlockBody, Header, PeerId, WithPeerId, H256};

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
