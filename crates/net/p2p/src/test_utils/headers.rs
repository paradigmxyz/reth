//! Testing support for headers related interfaces.

use crate::{
    download::DownloadClient,
    error::{DownloadError, DownloadResult, PeerRequestResult, RequestError},
    headers::{
        client::{HeadersClient, HeadersRequest},
        downloader::{HeaderDownloader, SyncTarget},
        error::HeadersDownloaderResult,
    },
    priority::Priority,
};
use alloy_primitives::Sealable;
use futures::{Future, FutureExt, Stream, StreamExt};
use reth_consensus::{test_utils::TestConsensus, Consensus};
use reth_eth_wire_types::HeadersDirection;
use reth_network_peers::{PeerId, WithPeerId};
use reth_primitives::{Header, SealedHeader};
use std::{
    fmt,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use tokio::sync::Mutex;

/// A test downloader which just returns the values that have been pushed to it.
#[derive(Debug)]
pub struct TestHeaderDownloader {
    client: TestHeadersClient,
    consensus: Arc<TestConsensus>,
    limit: u64,
    download: Option<TestDownload>,
    queued_headers: Vec<SealedHeader>,
    batch_size: usize,
}

impl TestHeaderDownloader {
    /// Instantiates the downloader with the mock responses
    pub const fn new(
        client: TestHeadersClient,
        consensus: Arc<TestConsensus>,
        limit: u64,
        batch_size: usize,
    ) -> Self {
        Self { client, consensus, limit, download: None, batch_size, queued_headers: Vec::new() }
    }

    fn create_download(&self) -> TestDownload {
        TestDownload {
            client: self.client.clone(),
            consensus: Arc::clone(&self.consensus),
            limit: self.limit,
            fut: None,
            buffer: vec![],
            done: false,
        }
    }
}

impl HeaderDownloader for TestHeaderDownloader {
    fn update_local_head(&mut self, _head: SealedHeader) {}

    fn update_sync_target(&mut self, _target: SyncTarget) {}

    fn set_batch_size(&mut self, limit: usize) {
        self.batch_size = limit;
    }
}

impl Stream for TestHeaderDownloader {
    type Item = HeadersDownloaderResult<Vec<SealedHeader>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if this.queued_headers.len() == this.batch_size {
                return Poll::Ready(Some(Ok(std::mem::take(&mut this.queued_headers))))
            }
            if this.download.is_none() {
                this.download = Some(this.create_download());
            }

            match ready!(this.download.as_mut().unwrap().poll_next_unpin(cx)) {
                None => return Poll::Ready(Some(Ok(std::mem::take(&mut this.queued_headers)))),
                Some(header) => this.queued_headers.push(header.unwrap()),
            }
        }
    }
}

type TestHeadersFut = Pin<Box<dyn Future<Output = PeerRequestResult<Vec<Header>>> + Sync + Send>>;

struct TestDownload {
    client: TestHeadersClient,
    consensus: Arc<TestConsensus>,
    limit: u64,
    fut: Option<TestHeadersFut>,
    buffer: Vec<SealedHeader>,
    done: bool,
}

impl TestDownload {
    fn get_or_init_fut(&mut self) -> &mut TestHeadersFut {
        if self.fut.is_none() {
            let request = HeadersRequest {
                limit: self.limit,
                direction: HeadersDirection::Rising,
                start: 0u64.into(), // ignored
            };
            let client = self.client.clone();
            self.fut = Some(Box::pin(client.get_headers(request)));
        }
        self.fut.as_mut().unwrap()
    }
}

impl fmt::Debug for TestDownload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestDownload")
            .field("client", &self.client)
            .field("consensus", &self.consensus)
            .field("limit", &self.limit)
            .field("buffer", &self.buffer)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl Stream for TestDownload {
    type Item = DownloadResult<SealedHeader>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            if let Some(header) = this.buffer.pop() {
                return Poll::Ready(Some(Ok(header)))
            } else if this.done {
                return Poll::Ready(None)
            }

            let empty = SealedHeader::default();
            if let Err(error) = this.consensus.validate_header_against_parent(&empty, &empty) {
                this.done = true;
                return Poll::Ready(Some(Err(DownloadError::HeaderValidation {
                    hash: empty.hash(),
                    number: empty.number,
                    error: Box::new(error),
                })))
            }

            match ready!(this.get_or_init_fut().poll_unpin(cx)) {
                Ok(resp) => {
                    // Skip head and seal headers
                    let mut headers = resp
                        .1
                        .into_iter()
                        .skip(1)
                        .map(|header| {
                            let sealed = header.seal_slow();
                            let (header, seal) = sealed.into_parts();
                            SealedHeader::new(header, seal)
                        })
                        .collect::<Vec<_>>();
                    headers.sort_unstable_by_key(|h| h.number);
                    headers.into_iter().for_each(|h| this.buffer.push(h));
                    this.done = true;
                    continue
                }
                Err(err) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(match err {
                        RequestError::Timeout => DownloadError::Timeout,
                        _ => DownloadError::RequestError(err),
                    })))
                }
            }
        }
    }
}

/// A test client for fetching headers
#[derive(Debug, Default, Clone)]
pub struct TestHeadersClient {
    responses: Arc<Mutex<Vec<Header>>>,
    error: Arc<Mutex<Option<RequestError>>>,
    request_attempts: Arc<AtomicU64>,
}

impl TestHeadersClient {
    /// Return the number of times client was polled
    pub fn request_attempts(&self) -> u64 {
        self.request_attempts.load(Ordering::SeqCst)
    }

    /// Adds headers to the set.
    pub async fn extend(&self, headers: impl IntoIterator<Item = Header>) {
        let mut lock = self.responses.lock().await;
        lock.extend(headers);
    }

    /// Clears the set.
    pub async fn clear(&self) {
        let mut lock = self.responses.lock().await;
        lock.clear();
    }

    /// Set response error
    pub async fn set_error(&self, err: RequestError) {
        let mut lock = self.error.lock().await;
        lock.replace(err);
    }
}

impl DownloadClient for TestHeadersClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        0
    }
}

impl HeadersClient for TestHeadersClient {
    type Output = TestHeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        let responses = self.responses.clone();
        let error = self.error.clone();

        self.request_attempts.fetch_add(1, Ordering::SeqCst);

        Box::pin(async move {
            if let Some(err) = &mut *error.lock().await {
                return Err(err.clone())
            }

            let mut lock = responses.lock().await;
            let len = lock.len().min(request.limit as usize);
            let resp = lock.drain(..len).collect();
            let with_peer_id = WithPeerId::from((PeerId::default(), resp));
            Ok(with_peer_id)
        })
    }
}
