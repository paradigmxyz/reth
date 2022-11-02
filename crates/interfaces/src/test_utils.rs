use crate::{
    consensus::{self, Consensus},
    p2p::headers::{
        client::{HeadersClient, HeadersRequest, HeadersResponse, HeadersStream},
        downloader::{DownloadError, Downloader},
    },
};
use std::{collections::HashSet, sync::Arc, time::Duration};

use reth_primitives::{Header, HeaderLocked, H256, H512, U256};
use reth_rpc_types::engine::ForkchoiceState;

use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

#[derive(Debug)]
/// A test downloader which just returns the values that have been pushed to it.
pub struct TestDownloader {
    result: Result<Vec<HeaderLocked>, DownloadError>,
}

impl TestDownloader {
    /// Instantiates the downloader with the mock responses
    pub fn new(result: Result<Vec<HeaderLocked>, DownloadError>) -> Self {
        Self { result }
    }
}

#[async_trait::async_trait]
impl Downloader for TestDownloader {
    type Consensus = TestConsensus;
    type Client = TestHeadersClient;

    fn timeout(&self) -> Duration {
        Duration::from_millis(1000)
    }

    fn consensus(&self) -> &Self::Consensus {
        unimplemented!()
    }

    fn client(&self) -> &Self::Client {
        unimplemented!()
    }

    async fn download(
        &self,
        _: &HeaderLocked,
        _: &ForkchoiceState,
    ) -> Result<Vec<HeaderLocked>, DownloadError> {
        self.result.clone()
    }
}

#[derive(Debug)]
/// A test client for fetching headers
pub struct TestHeadersClient {
    req_tx: mpsc::Sender<(u64, HeadersRequest)>,
    req_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<(u64, HeadersRequest)>>>,
    res_tx: broadcast::Sender<HeadersResponse>,
    res_rx: broadcast::Receiver<HeadersResponse>,
}

impl Default for TestHeadersClient {
    /// Construct a new test header downloader.
    fn default() -> Self {
        let (req_tx, req_rx) = mpsc::channel(1);
        let (res_tx, res_rx) = broadcast::channel(1);
        Self { req_tx, req_rx: Arc::new(tokio::sync::Mutex::new(req_rx)), res_tx, res_rx }
    }
}

impl TestHeadersClient {
    /// Helper for interacting with the environment on each request, allowing the client
    /// to also reply to messages.
    pub async fn on_header_request<T, F>(&self, mut count: usize, mut f: F) -> Vec<T>
    where
        F: FnMut(u64, HeadersRequest) -> T,
    {
        let mut rx = self.req_rx.lock().await;
        let mut results = vec![];
        while let Some((id, req)) = rx.recv().await {
            results.push(f(id, req));
            count -= 1;
            if count == 0 {
                break
            }
        }
        results
    }

    /// Helper for pushing responses to the client
    pub fn send_header_response(&self, id: u64, headers: Vec<Header>) {
        self.res_tx.send((id, headers).into()).expect("failed to send header response");
    }
}

#[async_trait::async_trait]
impl HeadersClient for TestHeadersClient {
    // noop
    async fn update_status(&self, _height: u64, _hash: H256, _td: H256) {}

    async fn send_header_request(&self, id: u64, request: HeadersRequest) -> HashSet<H512> {
        self.req_tx.send((id, request)).await.expect("failed to send request");
        HashSet::default()
    }

    async fn stream_headers(&self) -> HeadersStream {
        Box::pin(BroadcastStream::new(self.res_rx.resubscribe()).filter_map(|e| e.ok()))
    }
}

/// Consensus client impl for testing
#[derive(Debug)]
pub struct TestConsensus {
    /// Watcher over the forkchoice state
    channel: (watch::Sender<ForkchoiceState>, watch::Receiver<ForkchoiceState>),
    /// Flag whether the header validation should purposefully fail
    fail_validation: bool,
}

impl Default for TestConsensus {
    fn default() -> Self {
        Self {
            channel: watch::channel(ForkchoiceState {
                head_block_hash: H256::zero(),
                finalized_block_hash: H256::zero(),
                safe_block_hash: H256::zero(),
            }),
            fail_validation: false,
        }
    }
}

impl TestConsensus {
    /// Update the forkchoice state
    pub fn update_tip(&self, tip: H256) {
        let state = ForkchoiceState {
            head_block_hash: tip,
            finalized_block_hash: H256::zero(),
            safe_block_hash: H256::zero(),
        };
        self.channel.0.send(state).expect("updating forkchoice state failed");
    }

    /// Update the validation flag
    pub fn set_fail_validation(&mut self, val: bool) {
        self.fail_validation = val;
    }
}

#[async_trait::async_trait]
impl Consensus for TestConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        self.channel.1.clone()
    }

    fn validate_header(
        &self,
        _header: &HeaderLocked,
        _parent: &HeaderLocked,
    ) -> Result<(), consensus::Error> {
        if self.fail_validation {
            Err(consensus::Error::BaseFeeMissing)
        } else {
            Ok(())
        }
    }
}

/// Generate a range of random header. The parent hash of the first header
/// in the result will be equal to head
pub fn gen_random_header_range(rng: std::ops::Range<u64>, head: H256) -> Vec<HeaderLocked> {
    let mut headers = Vec::with_capacity(rng.end.saturating_sub(rng.start) as usize);
    for idx in rng {
        headers.push(gen_random_header(
            idx,
            Some(headers.last().map(|h: &HeaderLocked| h.hash()).unwrap_or(head)),
        ));
    }
    headers
}

/// Generate a random header
pub fn gen_random_header(number: u64, parent: Option<H256>) -> HeaderLocked {
    let header = reth_primitives::Header {
        number,
        nonce: rand::random(),
        difficulty: U256::from(rand::random::<u32>()),
        parent_hash: parent.unwrap_or_default(),
        ..Default::default()
    };
    header.lock()
}
