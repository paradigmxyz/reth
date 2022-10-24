use std::time::Duration;

use async_trait::async_trait;
use reth_interfaces::{
    consensus::Consensus,
    p2p::headers::{
        client::{HeadersClient, HeadersStream},
        downloader::{DownloadError, Downloader},
    },
};
use reth_primitives::{rpc::BlockId, HeaderLocked};
use reth_rpc_types::engine::ForkchoiceState;

/// Download headers in batches
#[derive(Debug)]
pub struct LinearDownloader<'a, C, H> {
    /// The consensus client
    consensus: &'a C,
    /// The headers client
    client: &'a H,
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: Duration,
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
    fn timeout(&self) -> Duration {
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
                    LinearDownloadResult::Batch(mut headers) => {
                        // TODO: fix
                        headers.extend_from_slice(&out);
                        out = headers;
                    }
                    LinearDownloadResult::Finished(mut headers) => {
                        // TODO: fix
                        headers.extend_from_slice(&out);
                        out = headers;
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
        stream: &'a mut HeadersStream,
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
                Some(header) if self.validate(header, &parent).is_ok() => {
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
    use super::*;
    use reth_interfaces::{
        p2p::headers::client::HeadersRequest,
        test_helpers::{TestConsensus, TestHeadersClient},
    };
    use reth_primitives::{rpc::BlockId, HeaderLocked, H256};
    use test_runner::LinearTestRunner;

    use assert_matches::assert_matches;
    use once_cell::sync::Lazy;
    use tokio::sync::oneshot::error::TryRecvError;

    static CONSENSUS: Lazy<TestConsensus> = Lazy::new(|| TestConsensus::default());
    static CONSENSUS_FAIL: Lazy<TestConsensus> = Lazy::new(|| {
        let mut consensus = TestConsensus::default();
        consensus.set_fail_validation(true);
        consensus
    });

    static CLIENT: Lazy<TestHeadersClient> = Lazy::new(|| TestHeadersClient::default());

    #[tokio::test]
    async fn download_timeout() {
        let runner = LinearTestRunner::new();
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
        let runner = LinearTestRunner::new();
        let retries = runner.retries;
        let rx = runner.run(&*CONSENSUS, &*CLIENT, HeaderLocked::default(), H256::zero());

        let mut num_of_reqs = 0;
        let mut last_req_id: Option<u64> = None;

        CLIENT
            .on_header_request(retries, |id, _req| {
                num_of_reqs += 1;
                last_req_id = Some(id);
                CLIENT.send_header_response(id.saturating_add(id % 2), vec![]);
            })
            .await;

        assert_eq!(num_of_reqs, retries);
        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::NoHeaderResponse { request_id })) if request_id == last_req_id.unwrap());
    }

    #[tokio::test]
    async fn download_propagates_consensus_validation_error() {
        let tip_parent = gen_random_header(1, None);
        let tip = gen_random_header(2, Some(tip_parent.hash()));

        let rx = LinearTestRunner::new().run(
            &*CONSENSUS_FAIL,
            &*CLIENT,
            HeaderLocked::default(),
            tip.hash(),
        );

        let requests = CLIENT.on_header_request(1, |id, req| (id, req)).await;
        let request = requests.last();
        assert_matches!(
            request,
            Some((_, HeadersRequest { start, .. }))
                if matches!(start, BlockId::Hash(hash) if *hash == tip.hash())
        );

        let request = request.unwrap();
        CLIENT.send_header_response(
            request.0,
            vec![tip_parent.clone().unlock(), tip.clone().unlock()],
        );

        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::HeaderValidation { hash, .. })) if hash == tip_parent.hash()
        );
    }

    #[tokio::test]
    async fn download_starts_with_chain_tip() {
        let head = gen_random_header(1, None);
        let tip = gen_random_header(2, Some(head.hash()));

        let mut rx = LinearTestRunner::new().run(&*CONSENSUS, &*CLIENT, head.clone(), tip.hash());

        CLIENT
            .on_header_request(1, |id, _req| {
                let mut corrupted_tip = tip.clone().unlock();
                corrupted_tip.nonce = rand::random();
                CLIENT.send_header_response(id, vec![corrupted_tip, head.clone().unlock()])
            })
            .await;
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        CLIENT
            .on_header_request(1, |id, _req| {
                CLIENT.send_header_response(id, vec![tip.clone().unlock(), head.clone().unlock()])
            })
            .await;

        let result = rx.await;
        assert_matches!(result, Ok(Ok(ref val)) if val.len() == 1);
        assert_eq!(*result.unwrap().unwrap().first().unwrap(), tip);
    }

    #[tokio::test]
    async fn download_returns_headers_asc() {
        let (start, end) = (100, 200);
        let head = gen_random_header(start, None);
        let headers = gen_block_range(start + 1..end, head.hash());
        let tip = headers.last().unwrap();

        let rx = LinearTestRunner::new().run(&*CONSENSUS, &*CLIENT, head.clone(), tip.hash());

        let mut idx = 0;
        let chunk_size = 10;
        let chunk_iter = headers.clone().into_iter().rev();
        // `usize::div_ceil` is unstable. ref: https://github.com/rust-lang/rust/issues/88581
        let count = (headers.len() + chunk_size - 1) / chunk_size;
        CLIENT
            .on_header_request(count + 1, |id, _req| {
                let mut chunk =
                    chunk_iter.clone().skip(chunk_size * idx).take(chunk_size).peekable();
                idx += 1;
                if chunk.peek().is_some() {
                    let headers: Vec<_> = chunk.map(|h| h.unlock()).collect();
                    CLIENT.send_header_response(id, headers);
                } else {
                    CLIENT.send_header_response(id, vec![head.clone().unlock()])
                }
            })
            .await;

        let result = rx.await;
        assert_matches!(result, Ok(Ok(_)));
        let result = result.unwrap().unwrap();
        assert_eq!(result, headers);
    }

    mod test_runner {
        use super::*;
        use reth_interfaces::{consensus::Consensus, p2p::headers::client::HeadersClient};
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

            pub(crate) fn run<C: Consensus, H: HeadersClient>(
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
                    request_timeout: Duration::from_millis(100),
                };
                tokio::spawn(async move {
                    let forkchoice = ForkchoiceState { head_block_hash: tip, ..Default::default() };
                    let result = downloader.download(&head, &forkchoice).await;
                    tx.send(result).expect("failed to forward download response");
                });
                rx
            }
        }
    }

    pub(crate) fn gen_block_range(rng: std::ops::Range<u64>, head: H256) -> Vec<HeaderLocked> {
        let mut headers = Vec::with_capacity(rng.end.saturating_sub(rng.start) as usize);
        for idx in rng {
            headers.push(gen_random_header(
                idx,
                Some(headers.last().map(|h: &HeaderLocked| h.hash()).unwrap_or(head)),
            ));
        }
        headers
    }

    pub(crate) fn gen_random_header(number: u64, parent: Option<H256>) -> HeaderLocked {
        let header = reth_primitives::Header {
            number,
            nonce: rand::random(),
            parent_hash: parent.unwrap_or_default(),
            ..Default::default()
        };
        header.lock()
    }
}
