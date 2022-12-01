use futures::{stream::Stream, FutureExt};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        error::{RequestError, RequestResult},
        headers::{
            client::{BlockHeaders, HeadersClient, HeadersRequest},
            downloader::{validate_header_download, HeaderBatchDownload, HeaderDownloader},
            error::DownloadError,
        },
        traits::BatchDownload,
    },
};
use reth_primitives::{HeadersDirection, SealedHeader, H256};
use reth_rpc_types::engine::ForkchoiceState;
use std::{
    borrow::Borrow,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Duration,
};

/// Download headers in batches
#[derive(Debug)]
pub struct LinearDownloader<C, H> {
    /// The consensus client
    consensus: Arc<C>,
    /// The headers client
    client: Arc<H>,
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: Duration,
    /// The number of retries for downloading
    pub request_retries: usize,
}

impl<C, H> HeaderDownloader for LinearDownloader<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    type Consensus = C;
    type Client = H;

    /// The request timeout
    fn timeout(&self) -> Duration {
        self.request_timeout
    }

    fn consensus(&self) -> &Self::Consensus {
        self.consensus.borrow()
    }

    fn client(&self) -> &Self::Client {
        self.client.borrow()
    }

    fn download(&self, head: SealedHeader, forkchoice: ForkchoiceState) -> HeaderBatchDownload<'_> {
        Box::pin(HeadersDownload {
            head,
            forkchoice,
            buffered: vec![],
            request: Default::default(),
            consensus: Arc::clone(&self.consensus),
            request_retries: self.request_retries,
            batch_size: self.batch_size,
            client: Arc::clone(&self.client),
        })
    }
}

impl<C: Consensus, H: HeadersClient> Clone for LinearDownloader<C, H> {
    fn clone(&self) -> Self {
        Self {
            consensus: Arc::clone(&self.consensus),
            client: Arc::clone(&self.client),
            batch_size: self.batch_size,
            request_timeout: self.request_timeout,
            request_retries: self.request_retries,
        }
    }
}

type HeadersFut = Pin<Box<dyn Future<Output = RequestResult<BlockHeaders>> + Send>>;

/// A retryable future that returns a list of [`BlockHeaders`] on success.
struct HeadersRequestFuture {
    request: HeadersRequest,
    fut: HeadersFut,
    retries: usize,
    max_retries: usize,
}

impl HeadersRequestFuture {
    /// Returns true if the request can be retried.
    fn is_retryable(&self) -> bool {
        self.retries < self.max_retries
    }

    /// Increments the retry counter and returns whether the request can still be retried.
    fn inc_err(&mut self) -> bool {
        self.retries += 1;
        self.is_retryable()
    }
}

impl Future for HeadersRequestFuture {
    type Output = RequestResult<BlockHeaders>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().fut.poll_unpin(cx)
    }
}

/// An in progress headers download.
pub struct HeadersDownload<C, H> {
    /// The local head of the chain.
    head: SealedHeader,
    forkchoice: ForkchoiceState,
    /// Buffered results
    buffered: Vec<SealedHeader>,
    /// Contains the request that's currently in progress.
    ///
    /// TODO(mattsse): this could be converted into a `FuturesOrdered` where batching is done via
    /// `skip` so we don't actually need to know the start hash
    request: Option<HeadersRequestFuture>,
    /// Downloader used to issue new requests.
    consensus: Arc<C>,
    /// Downloader used to issue new requests.
    client: Arc<H>,
    /// The number of headers to request in one call
    batch_size: u64,
    /// The number of retries for downloading
    request_retries: usize,
}

impl<C, H> HeadersDownload<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    /// Returns the start hash for a new request.
    fn request_start(&self) -> H256 {
        self.buffered.last().map_or(self.forkchoice.head_block_hash, |h| h.parent_hash)
    }

    fn headers_request(&self) -> HeadersRequest {
        HeadersRequest {
            start: self.request_start().into(),
            limit: self.batch_size,
            direction: HeadersDirection::Rising,
        }
    }

    /// Tries to fuse the future with a new request
    ///
    /// Returns an `Err` if the request exhausted all retries
    fn try_fuse_request_fut(&self, fut: &mut HeadersRequestFuture) -> Result<(), ()> {
        if !fut.inc_err() {
            return Err(())
        }
        let req = self.headers_request();
        fut.request = req.clone();
        let client = Arc::clone(&self.client);
        fut.fut = Box::pin(async move { client.get_headers(req).await });
        Ok(())
    }

    /// Validate whether the header is valid in relation to it's parent
    ///
    /// Returns Ok(false) if the
    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), DownloadError> {
        validate_header_download(&self.consensus, header, parent)?;
        Ok(())
    }
}

impl<C, H> Future for HeadersDownload<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    type Output = Result<Vec<SealedHeader>, DownloadError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        'outer: loop {
            let mut fut = match this.request.take() {
                Some(fut) => fut,
                None => {
                    // queue in the first request
                    let client = Arc::clone(&this.client);
                    let req = this.headers_request();
                    HeadersRequestFuture {
                        request: req.clone(),
                        fut: Box::pin(async move { client.get_headers(req).await }),
                        retries: 0,
                        max_retries: this.request_retries,
                    }
                }
            };

            match ready!(fut.poll_unpin(cx)) {
                Ok(resp) => {
                    let mut headers = resp.0;
                    headers.sort_unstable_by_key(|h| h.number);

                    if headers.is_empty() {
                        if this.try_fuse_request_fut(&mut fut).is_err() {
                            return Poll::Ready(Err(RequestError::BadResponse.into()))
                        } else {
                            this.request = Some(fut);
                            continue
                        }
                    }

                    // Iterate headers in reverse
                    for parent in headers.into_iter().rev() {
                        let parent = parent.seal();

                        if this.head.hash() == parent.hash() {
                            // We've reached the target
                            let headers =
                                std::mem::take(&mut this.buffered).into_iter().rev().collect();
                            return Poll::Ready(Ok(headers))
                        }

                        if let Some(header) = this.buffered.last() {
                            match this.validate(header, &parent) {
                                Ok(_) => {
                                    // record new parent
                                    this.buffered.push(parent);
                                }
                                Err(err) => {
                                    if this.try_fuse_request_fut(&mut fut).is_err() {
                                        return Poll::Ready(Err(err))
                                    }
                                    this.request = Some(fut);
                                    continue 'outer
                                }
                            }
                        } else {
                            // The buffer is empty and the first header does not match the tip,
                            // discard
                            if parent.hash() != this.forkchoice.head_block_hash {
                                if this.try_fuse_request_fut(&mut fut).is_err() {
                                    return Poll::Ready(Err(RequestError::BadResponse.into()))
                                }
                                this.request = Some(fut);
                                continue 'outer
                            }
                            this.buffered.push(parent);
                        }
                    }
                }
                Err(err) => {
                    if this.try_fuse_request_fut(&mut fut).is_err() {
                        return Poll::Ready(Err(DownloadError::RequestError(err)))
                    }
                    this.request = Some(fut);
                }
            }
        }
    }
}

impl<C, H> Stream for HeadersDownload<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    type Item = Result<SealedHeader, DownloadError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

impl<C, H> BatchDownload for HeadersDownload<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    type Ok = SealedHeader;
    type Error = DownloadError;

    fn into_stream_unordered(self) -> Box<dyn Stream<Item = Result<Self::Ok, Self::Error>>> {
        Box::new(self)
    }
}

/// The builder for [LinearDownloader] with
/// some default settings
#[derive(Debug)]
pub struct LinearDownloadBuilder {
    /// The batch size per one request
    batch_size: u64,
    /// A single request timeout
    request_timeout: Duration,
    /// The number of retries for downloading
    request_retries: usize,
}

impl Default for LinearDownloadBuilder {
    fn default() -> Self {
        Self { batch_size: 100, request_timeout: Duration::from_millis(100), request_retries: 5 }
    }
}

impl LinearDownloadBuilder {
    /// Set the request batch size
    pub fn batch_size(mut self, size: u64) -> Self {
        self.batch_size = size;
        self
    }

    /// Set the request timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set the number of retries per request
    pub fn retries(mut self, retries: usize) -> Self {
        self.request_retries = retries;
        self
    }

    /// Build [LinearDownloader] with provided consensus
    /// and header client implementations
    pub fn build<C: Consensus, H: HeadersClient>(
        self,
        consensus: Arc<C>,
        client: Arc<H>,
    ) -> LinearDownloader<C, H> {
        LinearDownloader {
            consensus,
            client,
            batch_size: self.batch_size,
            request_timeout: self.request_timeout,
            request_retries: self.request_retries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use reth_interfaces::test_utils::{TestConsensus, TestHeadersClient};
    use reth_primitives::SealedHeader;

    static CONSENSUS: Lazy<Arc<TestConsensus>> = Lazy::new(|| Arc::new(TestConsensus::default()));

    fn child_header(parent: &SealedHeader) -> SealedHeader {
        let mut child = parent.as_ref().clone();
        child.number += 1;
        child.parent_hash = parent.hash_slow();
        let hash = child.hash_slow();
        SealedHeader::new(child, hash)
    }

    #[tokio::test]
    async fn download_empty() {
        let client = Arc::new(TestHeadersClient::default());
        let downloader =
            LinearDownloadBuilder::default().build(CONSENSUS.clone(), Arc::clone(&client));

        let result = downloader.download(SealedHeader::default(), ForkchoiceState::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn download_at_fork_head() {
        let client = Arc::new(TestHeadersClient::default());
        let downloader = LinearDownloadBuilder::default()
            .batch_size(3)
            .build(CONSENSUS.clone(), Arc::clone(&client));

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let fork = ForkchoiceState { head_block_hash: p0.hash_slow(), ..Default::default() };

        let result = downloader.download(p0, fork).await;
        let headers = result.unwrap();
        assert!(headers.is_empty());
    }

    #[tokio::test]
    async fn download_exact() {
        let client = Arc::new(TestHeadersClient::default());
        let downloader = LinearDownloadBuilder::default()
            .batch_size(3)
            .build(CONSENSUS.clone(), Arc::clone(&client));

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let fork = ForkchoiceState { head_block_hash: p0.hash_slow(), ..Default::default() };

        let result = downloader.download(p3, fork).await;
        let headers = result.unwrap();
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[0], p2);
        assert_eq!(headers[1], p1);
        assert_eq!(headers[2], p0);
    }
}
