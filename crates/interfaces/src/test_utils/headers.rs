//! Testing support for headers related interfaces.
use crate::{
    consensus::{self, Consensus},
    p2p::{
        download::DownloadClient,
        error::{DownloadError, DownloadResult, PeerRequestResult, RequestError},
        headers::{
            client::{HeadersClient, HeadersRequest, StatusUpdater},
            downloader::{validate_header_download, HeaderDownloader, SyncTarget},
        },
        priority::Priority,
    },
};
use futures::{Future, FutureExt, Stream, StreamExt};
use reth_eth_wire::BlockHeaders;
use reth_primitives::{
    BlockHash, BlockNumber, Header, HeadersDirection, PeerId, SealedBlock, SealedHeader, H256,
};
use reth_rpc_types::engine::ForkchoiceState;
use std::{
    fmt,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use tokio::sync::{watch, Mutex};

/// A test downloader which just returns the values that have been pushed to it.
#[derive(Debug)]
pub struct TestHeaderDownloader {
    client: Arc<TestHeadersClient>,
    consensus: Arc<TestConsensus>,
    limit: u64,
    download: Option<TestDownload>,
    queued_headers: Vec<SealedHeader>,
    batch_size: usize,
}

impl TestHeaderDownloader {
    /// Instantiates the downloader with the mock responses
    pub fn new(
        client: Arc<TestHeadersClient>,
        consensus: Arc<TestConsensus>,
        limit: u64,
        batch_size: usize,
    ) -> Self {
        Self { client, consensus, limit, download: None, batch_size, queued_headers: Vec::new() }
    }

    fn create_download(&self) -> TestDownload {
        TestDownload {
            client: Arc::clone(&self.client),
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

    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
        validate_header_download(&self.consensus, header, parent)?;
        Ok(())
    }
}

impl Stream for TestHeaderDownloader {
    type Item = Vec<SealedHeader>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if this.queued_headers.len() == this.batch_size {
                return Poll::Ready(Some(std::mem::take(&mut this.queued_headers)))
            }
            if this.download.is_none() {
                this.download.insert(this.create_download());
            }

            match ready!(this.download.as_mut().unwrap().poll_next_unpin(cx)) {
                None => return Poll::Ready(Some(std::mem::take(&mut this.queued_headers))),
                Some(header) => this.queued_headers.push(header.unwrap()),
            }
        }
    }
}

type TestHeadersFut = Pin<Box<dyn Future<Output = PeerRequestResult<BlockHeaders>> + Send>>;

struct TestDownload {
    client: Arc<TestHeadersClient>,
    consensus: Arc<TestConsensus>,
    limit: u64,
    fut: Option<TestHeadersFut>,
    buffer: Vec<SealedHeader>,
    done: bool,
}

/// SAFETY: All the mutations are performed through an exclusive reference on `poll`
unsafe impl Sync for TestDownload {}

impl TestDownload {
    fn get_or_init_fut(&mut self) -> &mut TestHeadersFut {
        if self.fut.is_none() {
            let request = HeadersRequest {
                limit: self.limit,
                direction: HeadersDirection::Rising,
                start: reth_primitives::BlockHashOrNumber::Number(0), // ignored
            };
            let client = Arc::clone(&self.client);
            self.fut = Some(Box::pin(async move { client.get_headers(request).await }));
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
            if let Err(error) = this.consensus.validate_header(&empty, &empty) {
                this.done = true;
                return Poll::Ready(Some(Err(DownloadError::HeaderValidation {
                    hash: empty.hash(),
                    error,
                })))
            }

            match ready!(this.get_or_init_fut().poll_unpin(cx)) {
                Ok(resp) => {
                    // Skip head and seal headers
                    let mut headers =
                        resp.1 .0.into_iter().skip(1).map(|h| h.seal()).collect::<Vec<_>>();
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
#[derive(Debug, Default)]
pub struct TestHeadersClient {
    responses: Arc<Mutex<Vec<Header>>>,
    error: Arc<Mutex<Option<RequestError>>>,
    request_attempts: AtomicU64,
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

#[async_trait::async_trait]
impl HeadersClient for TestHeadersClient {
    async fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> PeerRequestResult<BlockHeaders> {
        self.request_attempts.fetch_add(1, Ordering::SeqCst);
        if let Some(err) = &mut *self.error.lock().await {
            return Err(err.clone())
        }

        let mut lock = self.responses.lock().await;
        let len = lock.len().min(request.limit as usize);
        let resp = lock.drain(..len).collect();
        return Ok((PeerId::default(), BlockHeaders(resp)).into())
    }
}

/// Consensus engine implementation for testing
#[derive(Debug)]
pub struct TestConsensus {
    /// Watcher over the forkchoice state
    channel: (watch::Sender<ForkchoiceState>, watch::Receiver<ForkchoiceState>),
    /// Flag whether the header validation should purposefully fail
    fail_validation: AtomicBool,
}

impl Default for TestConsensus {
    fn default() -> Self {
        Self {
            channel: watch::channel(ForkchoiceState {
                head_block_hash: H256::zero(),
                finalized_block_hash: H256::zero(),
                safe_block_hash: H256::zero(),
            }),
            fail_validation: AtomicBool::new(false),
        }
    }
}

impl TestConsensus {
    /// Update the fork choice state
    pub fn update_tip(&self, tip: H256) {
        let state = ForkchoiceState {
            head_block_hash: tip,
            finalized_block_hash: H256::zero(),
            safe_block_hash: H256::zero(),
        };
        self.channel.0.send(state).expect("updating fork choice state failed");
    }

    /// Get the failed validation flag
    pub fn fail_validation(&self) -> bool {
        self.fail_validation.load(Ordering::SeqCst)
    }

    /// Update the validation flag
    pub fn set_fail_validation(&self, val: bool) {
        self.fail_validation.store(val, Ordering::SeqCst)
    }
}

/// Nil status updater for testing
#[derive(Debug, Clone, Default)]
pub struct TestStatusUpdater;

impl StatusUpdater for TestStatusUpdater {
    fn update_status(&self, _height: u64, _hash: H256, _total_difficulty: reth_primitives::U256) {}
}

#[async_trait::async_trait]
impl Consensus for TestConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        self.channel.1.clone()
    }

    fn validate_header(
        &self,
        _header: &SealedHeader,
        _parent: &SealedHeader,
    ) -> Result<(), consensus::Error> {
        if self.fail_validation() {
            Err(consensus::Error::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn pre_validate_block(&self, _block: &SealedBlock) -> Result<(), consensus::Error> {
        if self.fail_validation() {
            Err(consensus::Error::BaseFeeMissing)
        } else {
            Ok(())
        }
    }
    fn has_block_reward(&self, _block_num: BlockNumber) -> bool {
        true
    }
}
