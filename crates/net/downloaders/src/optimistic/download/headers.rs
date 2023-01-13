use futures::{Future, FutureExt};
use reth_eth_wire::BlockHeaders;
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        error::{DownloadError, DownloadResult, PeerRequestResult},
        headers::{
            client::{HeadersClient, HeadersRequest},
            downloader::validate_header_download,
        },
    },
};
use reth_primitives::{BlockHashOrNumber, HeadersDirection, SealedHeader, H256};
use std::{
    borrow::Borrow,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::*;

type HeadersFut = Pin<Box<dyn Future<Output = PeerRequestResult<BlockHeaders>> + Send>>;

/// The headers download.
pub(crate) struct HeadersDownload<H, C> {
    /// Pending request future.
    pub(crate) fut: Option<HeadersFut>,
    client: Arc<H>,
    consensus: Arc<C>,
    batch_size: u64,
    tip: H256,
    head: SealedHeader,
    earliest_header: Option<SealedHeader>,
}

impl<H, C> std::fmt::Debug for HeadersDownload<H, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeaderDownload")
            .field("tip", &self.tip)
            .field("head", &self.head)
            .field("earliest_header", &self.earliest_header)
            .field("batch_size", &self.batch_size)
            .finish()
    }
}

impl<H, C> HeadersDownload<H, C>
where
    H: HeadersClient + 'static,
    C: Consensus,
{
    /// Create new instance of download.
    pub(crate) fn new(
        client: Arc<H>,
        consensus: Arc<C>,
        tip: H256,
        head: SealedHeader,
        batch_size: u64,
    ) -> Self {
        Self { client, consensus, batch_size, tip, head, earliest_header: None, fut: None }
    }

    fn request(&self) -> HeadersRequest {
        HeadersRequest {
            start: BlockHashOrNumber::Hash(self.tip),
            limit: self.batch_size,
            direction: HeadersDirection::Falling,
        }
    }

    /// Initialize and return request future.
    fn init_fut(&self) -> HeadersFut {
        let client = Arc::clone(&self.client);
        let req = self.request();
        tracing::trace!(target: "downloader::headers", "Requesting headers {req:?}");
        Box::pin(async move { client.get_headers(req).await })
    }

    /// Drop the future.
    pub(crate) fn clear(&mut self) {
        self.fut = None;
    }

    /// Take existing or initialize new future
    fn take_or_init_fut(&mut self) -> HeadersFut {
        self.fut.take().unwrap_or_else(|| self.init_fut())
    }

    /// Advance the header download.
    fn advance(&mut self, earliest_header: SealedHeader) {
        self.tip = earliest_header.parent_hash;
        self.earliest_header = Some(earliest_header);
        self.fut = Some(self.init_fut());
    }

    /// Verifies the response
    fn verify(
        &mut self,
        response: PeerRequestResult<BlockHeaders>,
    ) -> DownloadResult<Vec<SealedHeader>> {
        match response {
            Ok(res) => {
                let mut headers = res.1 .0;
                headers.sort_unstable_by_key(|h| h.number);

                if headers.is_empty() {
                    return Err(DownloadError::EmptyResponse)
                }

                let mut result = Vec::with_capacity(headers.len());

                // Iterate headers in reverse
                for parent in headers.into_iter().rev() {
                    let parent = parent.seal();

                    if self.head.hash() == parent.hash() {
                        // We've reached the target, stop buffering headers
                        result.push(parent);
                        break
                    }

                    if let Some(ref header) = self.earliest_header {
                        // TODO: return any buffered headers
                        // Proceed to insert. If there is a validation error re-queue the future.
                        validate_header_download::<C>(self.consensus.borrow(), header, &parent)?;
                    } else if parent.hash() != self.tip {
                        // The buffer is empty and the first header does not match the
                        // tip, requeue the future
                        return Err(DownloadError::InvalidTip {
                            received: parent.hash(),
                            expected: self.tip,
                        })
                    }

                    // Record new parent
                    result.push(parent);
                }

                Ok(result)
            }
            // most likely a noop, because this error
            // would've been handled by the fetcher internally
            Err(err) => Err(err.into()),
        }
    }
}

// TODO: make this a stream?
impl<H, C> Future for HeadersDownload<H, C>
where
    H: HeadersClient + 'static,
    C: Consensus,
{
    type Output = Vec<SealedHeader>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            let mut fut = this.take_or_init_fut();
            let result = match fut.poll_unpin(cx) {
                Poll::Ready(res) => res,
                Poll::Pending => {
                    this.fut = Some(fut);
                    return Poll::Pending
                }
            };
            let peer_id = result.as_ref().map(|res| res.peer_id()).ok();
            match this.verify(result) {
                Ok(mut result) => {
                    // The result is not empty because [HeadersDownload::verify]
                    // return an error on empty response.
                    let earliest_header = result.last().expect("noop; qed");
                    if this.head.hash() != earliest_header.hash() {
                        // Advance the download
                        this.advance(earliest_header.clone());
                    } else {
                        // We are done, remove head, and return the result
                        result.pop();
                    }
                    return Poll::Ready(result)
                }
                Err(error) => {
                    // Penalize peer and retry the request
                    error!(target: "downloader::headers", request = ?this.request(), ?error, "error downloading data");
                    if let Some(peer_id) = peer_id {
                        this.client.report_bad_message(peer_id);
                    }
                    this.fut = Some(this.init_fut());
                }
            }
        }
    }
}
