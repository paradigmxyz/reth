use super::downloader::{DownloadError, Downloader};
use async_trait::async_trait;
use futures::{future::BoxFuture, stream::BoxStream, Future, FutureExt};
use rand::Rng;
use reth_interfaces::{
    consensus::Consensus,
    stages::{HeaderRequest, HeadersClient, MessageStream},
};
use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256, H512};
use reth_rpc_types::engine::ForkchoiceState;
use std::{
    collections::HashSet,
    ops::DerefMut,
    pin::Pin,
    process::Output,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Sleep};
use tokio_stream::Stream;

/// Download headers in batches
#[derive(Debug)]
pub struct LinearDownloader<'a, C: Consensus> {
    consensus: &'a C,
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: u64,
    /// The number of retries for downloading
    pub request_retries: usize,
}

#[async_trait]
impl<'a, C: Consensus> Downloader for LinearDownloader<'a, C> {
    type Consensus = C;

    fn consensus(&self) -> &Self::Consensus {
        self.consensus
    }

    /// The request timeout
    fn timeout(&self) -> u64 {
        self.request_timeout
    }

    /// Download headers in batches with retries.
    /// Returns the header collection in sorted ascending order
    async fn download(
        &self,
        client: Arc<dyn HeadersClient>,
        head: &HeaderLocked,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<HeaderLocked>, DownloadError> {
        let mut stream = client.stream_headers().await;
        let mut retries = self.request_retries;

        // Header order will be preserved during inserts
        let mut out = vec![];
        loop {
            let result = self
                .download_batch(&mut stream, client.clone(), forkchoice, head, out.get(0))
                .await;
            match result {
                Ok(result) => match result {
                    LinearDownloadResult::Batch(headers) => {
                        // TODO: Should this instead be?
                        // headers.extend_from_slice(&out);
                        // out = headers;
                        out.extend_from_slice(&headers);
                    }
                    LinearDownloadResult::Finished(headers) => {
                        // TODO: Should this instead be?
                        // headers.extend_from_slice(&out);
                        // out = headers;
                        out.extend_from_slice(&headers);
                        return Ok(out)
                    }
                    LinearDownloadResult::Ignore => (),
                },
                Err(e) if e.is_retryable() && retries > 1 => {
                    retries -= 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// The intermediate download result
#[derive(Debug)]
pub enum LinearDownloadResult {
    /// Downloaded last batch up to tip
    Finished(Vec<HeaderLocked>),
    /// Downloaded batch
    Batch(Vec<HeaderLocked>),
    /// Ignore this batch
    Ignore,
}

impl<'a, C: Consensus> LinearDownloader<'a, C> {
    async fn download_batch(
        &'a self,
        stream: &'a mut MessageStream<(u64, Vec<Header>)>,
        client: Arc<dyn HeadersClient>,
        forkchoice: &'a ForkchoiceState,
        head: &'a HeaderLocked,
        earliest: Option<&HeaderLocked>,
    ) -> Result<LinearDownloadResult, DownloadError> {
        // Request headers starting from tip or earliest cached
        let start = earliest.map_or(forkchoice.head_block_hash, |h| h.parent_hash);
        let mut headers = self
            .download_headers(stream, client.clone(), BlockId::Hash(start), self.batch_size)
            .await?;
        headers.sort_unstable_by_key(|h| h.number);

        let mut out = Vec::with_capacity(headers.len());
        // Iterate headers in reverse
        for parent in headers.into_iter().rev() {
            let parent = parent.lock();

            if head.hash() == parent.hash() {
                // We've reached the target
                return Ok(LinearDownloadResult::Finished(out))
            }

            match out.first().or(earliest) {
                Some(header) if !self.validate(header, &parent)? => {
                    return Ok(LinearDownloadResult::Ignore)
                }
                // The buffer is empty and the first header does not match the tip, discard
                // TODO: penalize the peer?
                None if parent.hash() != forkchoice.head_block_hash => {
                    return Ok(LinearDownloadResult::Ignore)
                }
                _ => (),
            };

            out.insert(0, parent);
        }

        Ok(LinearDownloadResult::Batch(out))
    }
}

#[cfg(test)]
mod tests {
    use super::{super::stage::tests::test_utils, DownloadError, Downloader, LinearDownloader};
    use assert_matches::assert_matches;
    use rand::{self, Rng};
    use reth_interfaces::stages::HeaderRequest;
    use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256};
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc, oneshot::error::TryRecvError};
    use tokio_stream::{wrappers::ReceiverStream, StreamExt};

    use once_cell::sync::Lazy;
    use test_utils::TestConsensus;

    static CONSENSUS: Lazy<TestConsensus> = Lazy::new(|| TestConsensus::new());
    static CONSENSUS_FAIL: Lazy<TestConsensus> = Lazy::new(|| {
        let mut consensus = TestConsensus::new();
        consensus.set_fail_validation(true);
        consensus
    });

    #[tokio::test]
    async fn download_timeout() {
        let (req_tx, req_rx) = mpsc::channel(1);
        let (_res_tx, res_rx) = broadcast::channel(1);

        let runner = test_runner::LinearTestRunner::new();
        let retries = runner.retries;
        let rx = runner.run(
            &*CONSENSUS,
            test_utils::TestHeaderClient::new(req_tx, res_rx),
            HeaderLocked::default(),
            H256::zero(),
        );

        let requests = ReceiverStream::new(req_rx).collect::<Vec<_>>().await;
        assert_eq!(requests.len(), retries);
        assert_matches!(rx.await, Ok(Err(DownloadError::NoHeaderResponse { .. })));
    }

    #[tokio::test]
    async fn download_timeout_on_invalid_messages() {
        let (req_tx, req_rx) = mpsc::channel(1);
        let (res_tx, res_rx) = broadcast::channel(1);

        let runner = test_runner::LinearTestRunner::new();
        let retries = runner.retries;
        let rx = runner.run(
            &*CONSENSUS,
            test_utils::TestHeaderClient::new(req_tx, res_rx),
            HeaderLocked::default(),
            H256::zero(),
        );

        let mut num_of_reqs = 0;
        let mut last_req_id = None;
        let mut req_stream = ReceiverStream::new(req_rx);
        while let Some((id, _req)) = req_stream.next().await {
            // Since the receiving channel filters by id and message length -
            // randomize the input to the tested filter
            res_tx.send((id.saturating_add(id % 2), vec![])).expect("failed to send response");
            num_of_reqs += 1;
            last_req_id = Some(id);

            if num_of_reqs == retries {
                drop(res_tx);
                break
            }
        }

        assert_eq!(num_of_reqs, retries);
        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::NoHeaderResponse { request_id })) if request_id == last_req_id.unwrap()
        );
    }

    #[tokio::test]
    async fn download_propagates_consensus_validation_error() {
        let (req_tx, req_rx) = mpsc::channel(1);
        let (res_tx, res_rx) = broadcast::channel(1);

        let mut tip_parent = Header::default();
        tip_parent.nonce = rand::thread_rng().gen();
        tip_parent.number = 1;
        let parent_hash = tip_parent.hash_slow();

        let mut tip_header = Header::default();
        tip_header.number = 2;
        tip_header.nonce = rand::thread_rng().gen();
        tip_header.parent_hash = parent_hash;
        let chain_tip = tip_header.hash_slow();

        let runner = test_runner::LinearTestRunner::new();
        let rx = runner.run(
            &*CONSENSUS_FAIL,
            test_utils::TestHeaderClient::new(req_tx, res_rx),
            HeaderLocked::default(),
            chain_tip,
        );

        let mut stream = Box::pin(ReceiverStream::new(req_rx));
        let request = stream.next().await;
        assert_matches!(
            request,
            Some((_, HeaderRequest { start, .. }))
                if matches!(start, BlockId::Hash(hash) if hash == chain_tip)
        );

        let request = request.unwrap();
        res_tx.send((request.0, vec![tip_header, tip_parent])).expect("failed to send header");

        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::HeaderValidation { hash, .. })) if hash == parent_hash
        );
    }

    #[tokio::test]
    async fn download_starts_with_chain_tip() {
        let (req_tx, req_rx) = mpsc::channel(1);
        let (res_tx, res_rx) = broadcast::channel(1);

        let mut tip_parent = Header::default();
        tip_parent.nonce = rand::thread_rng().gen();
        tip_parent.number = 1;
        let parent_hash = tip_parent.hash_slow();

        let mut tip = Header::default();
        tip.parent_hash = parent_hash;
        tip.number = 2;
        tip.nonce = rand::thread_rng().gen();

        let runner = test_runner::LinearTestRunner::new();
        let mut rx = runner.run(
            &*CONSENSUS,
            test_utils::TestHeaderClient::new(req_tx, res_rx),
            tip_parent.clone().lock(),
            tip.hash_slow(),
        );

        let mut stream = ReceiverStream::new(req_rx);
        let request = stream.next().await.unwrap();
        let mut corrupted_tip = tip.clone();
        corrupted_tip.nonce = rand::thread_rng().gen();
        res_tx
            .send((request.0, vec![corrupted_tip, tip_parent.clone()]))
            .expect("failed to send header");
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        let request = stream.next().await.unwrap();
        res_tx
            .send((request.0, vec![tip.clone(), tip_parent.clone()]))
            .expect("failed to send header");

        let result = rx.await;
        assert_matches!(result, Ok(Ok(ref val)) if val.len() == 1);
        assert_eq!(*result.unwrap().unwrap().first().unwrap(), tip.lock());
    }

    mod test_runner {
        use super::*;
        use reth_interfaces::{consensus::Consensus, stages::HeadersClient};
        use reth_rpc_types::engine::ForkchoiceState;
        use tokio::sync::oneshot;

        type DownloadResult = Result<Vec<HeaderLocked>, DownloadError>;

        pub(crate) struct LinearTestRunner {
            pub(crate) retries: usize,
            test_ch: (oneshot::Sender<DownloadResult>, oneshot::Receiver<DownloadResult>),
        }

        impl LinearTestRunner {
            pub(crate) fn new() -> Self {
                Self { test_ch: oneshot::channel(), retries: 5 }
            }

            pub(crate) fn run<'a, C: Consensus>(
                self,
                consensus: &'static C,
                client: impl HeadersClient + 'static,
                head: HeaderLocked,
                tip: H256,
            ) -> oneshot::Receiver<DownloadResult> {
                let (tx, rx) = self.test_ch;
                let downloader = LinearDownloader {
                    request_retries: self.retries,
                    batch_size: 100,
                    consensus,
                    request_timeout: 3,
                };
                tokio::spawn(async move {
                    let mut forkchoice = ForkchoiceState::default();
                    forkchoice.head_block_hash = tip;
                    let result = downloader.download(Arc::new(client), &head, &forkchoice).await;
                    tx.send(result).expect("failed to forward download response");
                });
                rx
            }
        }
    }
}
