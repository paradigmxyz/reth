//! Support for different download types.

use crate::{
    bodies::client::BodiesClient,
    download::DownloadClient,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_primitives::B256;

pub use futures::future::Either;

impl<A, B> DownloadClient for Either<A, B>
where
    A: DownloadClient,
    B: DownloadClient,
{
    fn report_bad_message(&self, peer_id: reth_network_types::PeerId) {
        match self {
            Either::Left(a) => a.report_bad_message(peer_id),
            Either::Right(b) => b.report_bad_message(peer_id),
        }
    }
    fn num_connected_peers(&self) -> usize {
        match self {
            Either::Left(a) => a.num_connected_peers(),
            Either::Right(b) => b.num_connected_peers(),
        }
    }
}

impl<A, B> BodiesClient for Either<A, B>
where
    A: BodiesClient,
    B: BodiesClient,
{
    type Output = Either<A::Output, B::Output>;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<B256>,
        priority: Priority,
    ) -> Self::Output {
        match self {
            Either::Left(a) => Either::Left(a.get_block_bodies_with_priority(hashes, priority)),
            Either::Right(b) => Either::Right(b.get_block_bodies_with_priority(hashes, priority)),
        }
    }
}

impl<A, B> HeadersClient for Either<A, B>
where
    A: HeadersClient,
    B: HeadersClient,
{
    type Output = Either<A::Output, B::Output>;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        priority: Priority,
    ) -> Self::Output {
        match self {
            Either::Left(a) => Either::Left(a.get_headers_with_priority(request, priority)),
            Either::Right(b) => Either::Right(b.get_headers_with_priority(request, priority)),
        }
    }
}
