use super::downloader::{DownloadError, Downloader};
use async_trait::async_trait;
use reth_interfaces::{
    consensus::Consensus,
    stages::{HeadersClient, MessageStream},
};
use reth_primitives::{rpc::BlockId, Header, HeaderLocked};
use reth_rpc_types::engine::ForkchoiceState;

/// Download headers in batches
#[derive(Debug)]
pub struct LinearDownloader<'a, C: Consensus, H: HeadersClient> {
    /// The consensus client
    consensus: &'a C,
    /// The headers client
    client: &'a H,
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: u64,
    /// The number of retries for downloading
    pub request_retries: usize,
}

#[async_trait]
impl<'a, C: Consensus, H: HeadersClient> Downloader for LinearDownloader<'a, C, H> {
    type Consensus = C;
    type Client = H;

    fn consensus(&self) -> &Self::Consensus {
        self.consensus
    }

    fn client(&self) -> &Self::Client {
        self.client
    }

    /// The request timeout
    fn timeout(&self) -> u64 {
        self.request_timeout
    }

    /// Download headers in batches with retries.
    /// Returns the header collection in sorted ascending order
    async fn download(
        &self,
        head: &HeaderLocked,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<HeaderLocked>, DownloadError> {
        let mut stream = self.client().stream_headers().await;
        let mut retries = self.request_retries;

        // Header order will be preserved during inserts
        let mut out = vec![];
        loop {
            let result = self.download_batch(&mut stream, forkchoice, head, out.get(0)).await;
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

impl<'a, C: Consensus, H: HeadersClient> LinearDownloader<'a, C, H> {
    async fn download_batch(
        &'a self,
        stream: &'a mut MessageStream<(u64, Vec<Header>)>,
        forkchoice: &'a ForkchoiceState,
        head: &'a HeaderLocked,
        earliest: Option<&HeaderLocked>,
    ) -> Result<LinearDownloadResult, DownloadError> {
        // Request headers starting from tip or earliest cached
        let start = earliest.map_or(forkchoice.head_block_hash, |h| h.parent_hash);
        let mut headers =
            self.download_headers(stream, BlockId::Hash(start), self.batch_size).await?;
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
    use super::{
        super::stage::tests::test_utils::{TestConsensus, TestHeaderClient},
        DownloadError, Downloader, LinearDownloader,
    };
    use assert_matches::assert_matches;
    use once_cell::sync::Lazy;
    use rand::Rng;
    use reth_interfaces::stages::HeaderRequest;
    use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256};
    use tokio::sync::oneshot::error::TryRecvError;

    static CONSENSUS: Lazy<TestConsensus> = Lazy::new(|| TestConsensus::new());
    static CONSENSUS_FAIL: Lazy<TestConsensus> = Lazy::new(|| {
        let mut consensus = TestConsensus::new();
        consensus.set_fail_validation(true);
        consensus
    });

    static CLIENT: Lazy<TestHeaderClient> = Lazy::new(|| TestHeaderClient::new());

    #[tokio::test]
    async fn download_timeout() {
        let runner = test_runner::LinearTestRunner::new();
        let retries = runner.retries;
        let rx = runner.run(&*CONSENSUS, &*CLIENT, HeaderLocked::default(), H256::zero());

        let mut requests = vec![];
        CLIENT
            .on_header_request(retries, |_id, req| {
                requests.push(req);
            })
            .await;
        assert_eq!(requests.len(), retries);
        assert_matches!(rx.await, Ok(Err(DownloadError::NoHeaderResponse { .. })));
    }

    #[tokio::test]
    async fn download_timeout_on_invalid_messages() {
        let runner = test_runner::LinearTestRunner::new();
        let retries = runner.retries;
        let rx = runner.run(&*CONSENSUS, &*CLIENT, HeaderLocked::default(), H256::zero());

        let mut num_of_reqs = 0;
        let mut last_req_id: Option<u64> = None;

        CLIENT
            .on_header_request(retries, |id, _req| {
                num_of_reqs += 1;
                last_req_id = Some(id);
                CLIENT.send_header_response(id.saturating_add(id % 2), vec![])
            })
            .await;

        assert_eq!(num_of_reqs, retries);
        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::NoHeaderResponse { request_id })) if request_id == last_req_id.unwrap());
    }

    #[tokio::test]
    async fn download_propagates_consensus_validation_error() {
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
        let rx = runner.run(&*CONSENSUS_FAIL, &*CLIENT, HeaderLocked::default(), chain_tip);

        let requests = CLIENT.on_header_request(1, |id, req| (id, req)).await;

        let request = requests.last();
        assert_matches!(
            request,
            Some((_, HeaderRequest { start, .. }))
                if matches!(start, BlockId::Hash(hash) if *hash == chain_tip)
        );

        let request = request.unwrap();
        CLIENT.send_header_response(request.0, vec![tip_header, tip_parent]);

        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::HeaderValidation { hash, .. })) if hash == parent_hash
        );
    }

    #[tokio::test]
    async fn download_starts_with_chain_tip() {
        let mut tip_parent = Header::default();
        tip_parent.nonce = rand::thread_rng().gen();
        tip_parent.number = 1;
        let parent_hash = tip_parent.hash_slow();

        let mut tip = Header::default();
        tip.parent_hash = parent_hash;
        tip.number = 2;
        tip.nonce = rand::thread_rng().gen();

        let runner = test_runner::LinearTestRunner::new();
        let mut rx = runner.run(&*CONSENSUS, &*CLIENT, tip_parent.clone().lock(), tip.hash_slow());

        CLIENT
            .on_header_request(1, |id, _req| {
                let mut corrupted_tip = tip.clone();
                corrupted_tip.nonce = rand::thread_rng().gen();
                CLIENT.send_header_response(id, vec![corrupted_tip, tip_parent.clone()])
            })
            .await;
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        CLIENT
            .on_header_request(1, |id, _req| {
                CLIENT.send_header_response(id, vec![tip.clone(), tip_parent.clone()])
            })
            .await;

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

            pub(crate) fn run<'a, C: Consensus, H: HeadersClient>(
                self,
                consensus: &'static C,
                client: &'static H,
                head: HeaderLocked,
                tip: H256,
            ) -> oneshot::Receiver<DownloadResult> {
                let (tx, rx) = self.test_ch;
                let downloader = LinearDownloader {
                    consensus,
                    client,
                    request_retries: self.retries,
                    batch_size: 100,
                    request_timeout: 3,
                };
                tokio::spawn(async move {
                    let mut forkchoice = ForkchoiceState::default();
                    forkchoice.head_block_hash = tip;
                    let result = downloader.download(&head, &forkchoice).await;
                    tx.send(result).expect("failed to forward download response");
                });
                rx
            }
        }
    }
}
