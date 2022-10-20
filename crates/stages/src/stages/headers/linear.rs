use super::downloader::{DownloadError, Downloader};
use async_trait::async_trait;
use futures::{future::BoxFuture, stream::BoxStream, Future, FutureExt};
use pin_project::pin_project;
use pin_project_lite::pin_project as pin_project_lite;
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
pub struct LinearDownloader {
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: u64,
    /// The number of retries for downloading
    pub request_retries: usize,
}

#[async_trait]
impl Downloader for LinearDownloader {
    /// The request timeout
    fn timeout(&self) -> u64 {
        self.request_timeout
    }

    /// Download headers in batches with retries.
    /// Returns the header collection in sorted ascending order
    async fn download(
        &self,
        client: Arc<dyn HeadersClient>,
        consensus: Arc<dyn Consensus>,
        head: &HeaderLocked,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<HeaderLocked>, DownloadError> {
        let mut stream = client.stream_headers().await;
        let mut retries = self.request_retries;

        // Header order will be preserved during inserts
        let mut out = vec![];
        loop {
            let result = self
                .download_batch(
                    &mut stream,
                    client.clone(),
                    consensus.clone(),
                    forkchoice,
                    head,
                    out.get(0),
                )
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

impl LinearDownloader {
    async fn download_batch<'a>(
        &'a self,
        stream: &'a mut MessageStream<(u64, Vec<Header>)>,
        client: Arc<dyn HeadersClient>,
        consensus: Arc<dyn Consensus>,
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
                Some(header) if !self.validate(consensus.clone(), header, &parent)? => {
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

mod linear_stream {
    use super::*;

    pin_project_lite! {
        pub(crate) struct LinearDownloadStream<'a, S: Stream<Item = (u64, Vec<Header>)>> {
            #[pin]
            stream: &'a mut S,
            #[pin]
            state: LinearStreamState<'a>,
            client: &'a Arc<dyn HeadersClient>,
            consensus: &'a Arc<dyn Consensus>,
            tip: H256,
            head: H256,
            earliest: Option<HeaderLocked>,
            retries: usize,
        }
    }

    impl<'a, S: Stream<Item = (u64, Vec<Header>)> + Unpin> LinearDownloadStream<'a, S> {
        pub(crate) fn new(
            stream: &'a mut S,
            client: &'a Arc<dyn HeadersClient>,
            consensus: &'a Arc<dyn Consensus>,
            tip: H256,
            head: H256,
            retries: usize,
        ) -> Self {
            Self {
                stream,
                state: LinearStreamState::Prepare,
                tip,
                head,
                client,
                consensus,
                earliest: None,
                retries,
            }
        }
    }

    enum LinearStreamState<'a> {
        Prepare,
        PollRequest(u64, BoxFuture<'a, HashSet<H512>>),
        PollHeaders(u64),
        Done,
    }

    impl<'a, S: Stream<Item = (u64, Vec<Header>)> + Unpin + Send> Stream
        for LinearDownloadStream<'a, S>
    {
        type Item = Result<Vec<HeaderLocked>, DownloadError>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut this = self.project();
            match *this.state {
                LinearStreamState::Prepare => {
                    let request_id = rand::thread_rng().gen();
                    let request =
                        HeaderRequest { start: BlockId::Hash(*this.tip), limit: 1, reverse: true };
                    this.state.set(LinearStreamState::PollRequest(
                        request_id,
                        this.client.send_header_request(request_id, request),
                    ));
                    Poll::Pending
                }
                LinearStreamState::PollRequest(req_id, ref mut fut) => match fut.poll_unpin(cx) {
                    Poll::Ready(_peers) => {
                        this.state.set(LinearStreamState::PollHeaders(req_id));
                        Poll::Pending
                    }
                    Poll::Pending => Poll::Pending,
                },
                LinearStreamState::PollHeaders(req_id) => match this.stream.poll_next(cx) {
                    Poll::Ready(Some((id, mut headers))) if id == req_id && !headers.is_empty() => {
                        headers.sort_unstable_by_key(|h| h.number);

                        this.state.set(LinearStreamState::Prepare);
                        let mut out = Vec::with_capacity(headers.len());
                        for parent in headers.into_iter().rev() {
                            let parent = parent.lock();
                            if *this.head == parent.hash() {
                                this.state.set(LinearStreamState::Done);
                                break
                            }

                            match out.first().or(this.earliest.as_ref()) {
                                Some(header) => {
                                    if !(parent.hash() == header.parent_hash &&
                                        parent.number + 1 == header.number)
                                    {
                                        return Poll::Pending
                                    }

                                    if let Err(e) = this.consensus.validate_header(&header, &parent)
                                    {
                                        return Poll::Ready(Some(Err(
                                            DownloadError::HeaderValidation {
                                                hash: parent.hash(),
                                                details: e.to_string(),
                                            },
                                        )))
                                    }
                                }
                                None if parent.hash() != *this.tip => return Poll::Pending,
                                _ => (),
                            };

                            out.insert(0, parent);
                        }
                        *this.earliest = Some(out.first().unwrap().clone());
                        Poll::Ready(Some(Ok(out)))
                    }
                    _ => Poll::Pending,
                },
                LinearStreamState::Done => Poll::Ready(None),
            }
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            (1, None)
        }
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

    #[tokio::test]
    async fn download_timeout() {
        let (req_tx, req_rx) = mpsc::channel(1);
        let (_res_tx, res_rx) = broadcast::channel(1);

        let runner = test_runner::LinearTestRunner::new();
        let retries = runner.retries;
        let rx = runner.run(
            test_utils::TestConsensus::new(),
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
            test_utils::TestConsensus::new(),
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

        let mut consensus = test_utils::TestConsensus::new();
        consensus.set_fail_validation(true);

        let runner = test_runner::LinearTestRunner::new();
        let rx = runner.run(
            consensus,
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
            test_utils::TestConsensus::new(),
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

            pub(crate) fn run<'a>(
                self,
                consensus: impl Consensus + 'static,
                client: impl HeadersClient + 'static,
                head: HeaderLocked,
                tip: H256,
            ) -> oneshot::Receiver<DownloadResult> {
                let (tx, rx) = self.test_ch;
                let downloader = LinearDownloader {
                    request_retries: self.retries,
                    batch_size: 100,
                    request_timeout: 3,
                };
                tokio::spawn(async move {
                    let mut forkchoice = ForkchoiceState::default();
                    forkchoice.head_block_hash = tip;
                    let result = downloader
                        .download(Arc::new(client), Arc::new(consensus), &head, &forkchoice)
                        .await;
                    tx.send(result).expect("failed to forward download response");
                });
                rx
            }
        }
    }
}
