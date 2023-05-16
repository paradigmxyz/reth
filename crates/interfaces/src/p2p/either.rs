use crate::p2p::{
    bodies::client::BodiesClient,
    download::DownloadClient,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use futures::future::Either;
use reth_primitives::H256;

/// A downloader that combines two different downloaders/client implementations that have the same
/// associated types.
#[derive(Debug, Clone)]
pub enum EitherDownloader<A, B> {
    /// The first downloader variant
    Left(A),
    /// The second downloader variant
    Right(B),
}

impl<A, B> DownloadClient for EitherDownloader<A, B>
where
    A: DownloadClient,
    B: DownloadClient,
{
    fn report_bad_message(&self, peer_id: reth_primitives::PeerId) {
        match self {
            EitherDownloader::Left(a) => a.report_bad_message(peer_id),
            EitherDownloader::Right(b) => b.report_bad_message(peer_id),
        }
    }
    fn num_connected_peers(&self) -> usize {
        match self {
            EitherDownloader::Left(a) => a.num_connected_peers(),
            EitherDownloader::Right(b) => b.num_connected_peers(),
        }
    }
}

impl<A, B> BodiesClient for EitherDownloader<A, B>
where
    A: BodiesClient,
    B: BodiesClient,
{
    type Output = Either<A::Output, B::Output>;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        priority: Priority,
    ) -> Self::Output {
        match self {
            EitherDownloader::Left(a) => {
                Either::Left(a.get_block_bodies_with_priority(hashes, priority))
            }
            EitherDownloader::Right(b) => {
                Either::Right(b.get_block_bodies_with_priority(hashes, priority))
            }
        }
    }
}

impl<A, B> HeadersClient for EitherDownloader<A, B>
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
            EitherDownloader::Left(a) => {
                Either::Left(a.get_headers_with_priority(request, priority))
            }
            EitherDownloader::Right(b) => {
                Either::Right(b.get_headers_with_priority(request, priority))
            }
        }
    }
}
