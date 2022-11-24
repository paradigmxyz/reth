//! Testing support for headers related interfaces.
use crate::{
    consensus::{self, Consensus},
    p2p::{
        error::RequestResult,
        headers::{
            client::{HeadersClient, HeadersRequest},
            downloader::{HeaderBatchDownload, HeaderDownloader},
        },
    },
};
use reth_eth_wire::BlockHeaders;
use reth_primitives::{BlockLocked, Header, SealedHeader, H256, U256};
use reth_rpc_types::engine::ForkchoiceState;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{watch, Mutex};

/// A test downloader which just returns the values that have been pushed to it.
#[derive(Debug)]
pub struct TestHeaderDownloader {
    client: Arc<TestHeadersClient>,
    consensus: Arc<TestConsensus>,
}

impl TestHeaderDownloader {
    /// Instantiates the downloader with the mock responses
    pub fn new(client: Arc<TestHeadersClient>, consensus: Arc<TestConsensus>) -> Self {
        Self { client, consensus }
    }
}

#[async_trait::async_trait]
impl HeaderDownloader for TestHeaderDownloader {
    type Consensus = TestConsensus;
    type Client = TestHeadersClient;

    fn timeout(&self) -> Duration {
        Duration::from_millis(1000)
    }

    fn consensus(&self) -> &Self::Consensus {
        &self.consensus
    }

    fn client(&self) -> &Self::Client {
        &self.client
    }

    fn download(
        &self,
        _head: SealedHeader,
        _forkchoice: ForkchoiceState,
    ) -> HeaderBatchDownload<'_> {
        todo!()
    }

    // async fn download(
    //     &self,
    //     _: &SealedHeader,
    //     _: &ForkchoiceState,
    // ) -> Result<Vec<SealedHeader>, DownloadError> {
    //     // call consensus stub first. fails if the flag is set
    //     let empty = SealedHeader::default();
    //     self.consensus
    //         .validate_header(&empty, &empty)
    //         .map_err(|error| DownloadError::HeaderValidation { hash: empty.hash(), error })?;
    //
    //     let stream = self.client.stream_headers().await;
    //     let stream = stream.timeout(Duration::from_secs(1));
    //
    //     match Box::pin(stream).try_next().await {
    //         Ok(Some(res)) => {
    //             let mut headers = res.headers.iter().map(|h|
    // h.clone().seal()).collect::<Vec<_>>();             if !headers.is_empty() {
    //                 headers.sort_unstable_by_key(|h| h.number);
    //                 headers.remove(0); // remove head from response
    //                 headers.reverse();
    //             }
    //             Ok(headers)
    //         }
    //         _ => Err(DownloadError::Timeout { request_id: 0 }),
    //     }
    // }
}

/// A test client for fetching headers
#[derive(Debug, Default)]
pub struct TestHeadersClient {
    responses: Arc<Mutex<Vec<Header>>>,
}

impl TestHeadersClient {
    /// Adds headers to the set.
    pub async fn extend(&self, headers: impl IntoIterator<Item = Header>) {
        let mut lock = self.responses.lock().await;
        lock.extend(headers);
    }
}

#[async_trait::async_trait]
impl HeadersClient for TestHeadersClient {
    fn update_status(&self, _height: u64, _hash: H256, _td: U256) {}

    async fn get_headers(&self, request: HeadersRequest) -> RequestResult<BlockHeaders> {
        let mut lock = self.responses.lock().await;
        let len = lock.len().min(request.limit as usize);
        let resp = lock.drain(..len).collect();
        return Ok(BlockHeaders(resp))
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

    fn pre_validate_block(&self, _block: &BlockLocked) -> Result<(), consensus::Error> {
        if self.fail_validation() {
            Err(consensus::Error::BaseFeeMissing)
        } else {
            Ok(())
        }
    }
}
