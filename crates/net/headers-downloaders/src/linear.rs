use std::{borrow::Borrow, sync::Arc, time::Duration};

use async_trait::async_trait;
use reth_interfaces::{
    consensus::Consensus,
    p2p::headers::{
        client::{HeadersClient, HeadersStream},
        downloader::HeaderDownloader,
        error::DownloadError,
    },
};
use reth_primitives::SealedHeader;
use reth_rpc_types::engine::ForkchoiceState;

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

#[async_trait]
impl<C: Consensus, H: HeadersClient> HeaderDownloader for LinearDownloader<C, H> {
    type Consensus = C;
    type Client = H;

    fn consensus(&self) -> &Self::Consensus {
        self.consensus.borrow()
    }

    fn client(&self) -> &Self::Client {
        self.client.borrow()
    }

    /// The request timeout
    fn timeout(&self) -> Duration {
        self.request_timeout
    }

    /// Download headers in batches with retries.
    /// Returns the header collection in sorted descending
    /// order from chain tip to local head
    async fn download(
        &self,
        head: &SealedHeader,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<SealedHeader>, DownloadError> {
        let mut stream = self.client().stream_headers().await;
        let mut retries = self.request_retries;

        // Header order will be preserved during inserts
        let mut out = vec![];
        loop {
            let result = self.download_batch(&mut stream, forkchoice, head, out.last()).await;
            match result {
                Ok(result) => match result {
                    LinearDownloadResult::Batch(mut headers) => {
                        out.append(&mut headers);
                    }
                    LinearDownloadResult::Finished(mut headers) => {
                        out.append(&mut headers);
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
    Finished(Vec<SealedHeader>),
    /// Downloaded batch
    Batch(Vec<SealedHeader>),
    /// Ignore this batch
    Ignore,
}

impl<C: Consensus, H: HeadersClient> LinearDownloader<C, H> {
    async fn download_batch(
        &self,
        stream: &mut HeadersStream,
        forkchoice: &ForkchoiceState,
        head: &SealedHeader,
        earliest: Option<&SealedHeader>,
    ) -> Result<LinearDownloadResult, DownloadError> {
        // Request headers starting from tip or earliest cached
        let start = earliest.map_or(forkchoice.head_block_hash, |h| h.parent_hash);
        let mut headers = self.download_headers(stream, start.into(), self.batch_size).await?;
        headers.sort_unstable_by_key(|h| h.number);

        let mut out = Vec::with_capacity(headers.len());
        // Iterate headers in reverse
        for parent in headers.into_iter().rev() {
            let parent = parent.seal();

            if head.hash() == parent.hash() {
                // We've reached the target
                return Ok(LinearDownloadResult::Finished(out))
            }

            match out.last().or(earliest) {
                Some(header) => {
                    match self.validate(header, &parent) {
                        // ignore mismatched headers
                        Err(DownloadError::MismatchedHeaders { .. }) => {
                            return Ok(LinearDownloadResult::Ignore)
                        }
                        // propagate any other error if any
                        Err(e) => return Err(e),
                        // proceed to insert if validation is successful
                        _ => (),
                    };
                }
                // The buffer is empty and the first header does not match the tip, discard
                // TODO: penalize the peer?
                None if parent.hash() != forkchoice.head_block_hash => {
                    return Ok(LinearDownloadResult::Ignore)
                }
                _ => (),
            };

            out.push(parent);
        }

        Ok(LinearDownloadResult::Batch(out))
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
    use reth_interfaces::{
        p2p::headers::client::HeadersRequest,
        test_utils::{
            generators::{random_header, random_header_range},
            TestConsensus, TestHeadersClient,
        },
    };
    use reth_primitives::{BlockHashOrNumber, SealedHeader};

    use assert_matches::assert_matches;
    use once_cell::sync::Lazy;
    use serial_test::serial;
    use tokio::sync::oneshot::{self, error::TryRecvError};

    static CONSENSUS: Lazy<Arc<TestConsensus>> = Lazy::new(|| Arc::new(TestConsensus::default()));
    static CONSENSUS_FAIL: Lazy<Arc<TestConsensus>> = Lazy::new(|| {
        let consensus = TestConsensus::default();
        consensus.set_fail_validation(true);
        Arc::new(consensus)
    });

    static CLIENT: Lazy<Arc<TestHeadersClient>> =
        Lazy::new(|| Arc::new(TestHeadersClient::default()));

    #[tokio::test]
    #[serial]
    async fn download_timeout() {
        let retries = 5;
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let downloader = LinearDownloadBuilder::default()
                .retries(retries)
                .build(CONSENSUS.clone(), CLIENT.clone());
            let result =
                downloader.download(&SealedHeader::default(), &ForkchoiceState::default()).await;
            tx.send(result).expect("failed to forward download response");
        });

        let mut requests = vec![];
        CLIENT
            .on_header_request(retries, |_id, req| {
                requests.push(req);
            })
            .await;
        assert_eq!(requests.len(), retries);
        assert_matches!(rx.await, Ok(Err(DownloadError::Timeout { .. })));
    }

    #[tokio::test]
    #[serial]
    async fn download_timeout_on_invalid_messages() {
        let retries = 5;
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let downloader = LinearDownloadBuilder::default()
                .retries(retries)
                .build(CONSENSUS.clone(), CLIENT.clone());
            let result =
                downloader.download(&SealedHeader::default(), &ForkchoiceState::default()).await;
            tx.send(result).expect("failed to forward download response");
        });

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
            Ok(Err(DownloadError::Timeout { request_id })) if request_id == last_req_id.unwrap()
        );
    }

    #[tokio::test]
    #[serial]
    async fn download_propagates_consensus_validation_error() {
        let tip_parent = random_header(1, None);
        let tip = random_header(2, Some(tip_parent.hash()));
        let tip_hash = tip.hash();

        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let downloader =
                LinearDownloadBuilder::default().build(CONSENSUS_FAIL.clone(), CLIENT.clone());
            let forkchoice = ForkchoiceState { head_block_hash: tip_hash, ..Default::default() };
            let result = downloader.download(&SealedHeader::default(), &forkchoice).await;
            tx.send(result).expect("failed to forward download response");
        });

        let requests = CLIENT.on_header_request(1, |id, req| (id, req)).await;
        let request = requests.last();
        assert_matches!(
            request,
            Some((_, HeadersRequest { start, .. }))
                if matches!(start, BlockHashOrNumber::Hash(hash) if *hash == tip_hash)
        );

        let request = request.unwrap();
        CLIENT.send_header_response(
            request.0,
            vec![tip_parent.clone().unseal(), tip.clone().unseal()],
        );

        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::HeaderValidation { hash, .. })) if hash == tip_parent.hash()
        );
    }

    #[tokio::test]
    #[serial]
    async fn download_starts_with_chain_tip() {
        let head = random_header(1, None);
        let tip = random_header(2, Some(head.hash()));

        let tip_hash = tip.hash();
        let chain_head = head.clone();
        let (tx, mut rx) = oneshot::channel();
        tokio::spawn(async move {
            let downloader =
                LinearDownloadBuilder::default().build(CONSENSUS.clone(), CLIENT.clone());
            let forkchoice = ForkchoiceState { head_block_hash: tip_hash, ..Default::default() };
            let result = downloader.download(&chain_head, &forkchoice).await;
            tx.send(result).expect("failed to forward download response");
        });

        CLIENT
            .on_header_request(1, |id, _req| {
                let mut corrupted_tip = tip.clone().unseal();
                corrupted_tip.nonce = rand::random();
                CLIENT.send_header_response(id, vec![corrupted_tip, head.clone().unseal()])
            })
            .await;
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        CLIENT
            .on_header_request(1, |id, _req| {
                CLIENT.send_header_response(id, vec![tip.clone().unseal(), head.clone().unseal()])
            })
            .await;

        let result = rx.await;
        assert_matches!(result, Ok(Ok(ref val)) if val.len() == 1);
        assert_eq!(*result.unwrap().unwrap().first().unwrap(), tip);
    }

    #[tokio::test]
    #[serial]
    async fn download_returns_headers_desc() {
        let (start, end) = (100, 200);
        let head = random_header(start, None);
        let mut headers = random_header_range(start + 1..end, head.hash());
        headers.reverse();

        let tip_hash = headers.first().unwrap().hash();
        let chain_head = head.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let downloader =
                LinearDownloadBuilder::default().build(CONSENSUS.clone(), CLIENT.clone());
            let forkchoice = ForkchoiceState { head_block_hash: tip_hash, ..Default::default() };
            let result = downloader.download(&chain_head, &forkchoice).await;
            tx.send(result).expect("failed to forward download response");
        });

        let mut idx = 0;
        let chunk_size = 10;
        // `usize::div_ceil` is unstable. ref: https://github.com/rust-lang/rust/issues/88581
        let count = (headers.len() + chunk_size - 1) / chunk_size;
        CLIENT
            .on_header_request(count + 1, |id, _req| {
                let mut chunk =
                    headers.iter().skip(chunk_size * idx).take(chunk_size).cloned().peekable();
                idx += 1;
                if chunk.peek().is_some() {
                    let headers: Vec<_> = chunk.map(|h| h.unseal()).collect();
                    CLIENT.send_header_response(id, headers);
                } else {
                    CLIENT.send_header_response(id, vec![head.clone().unseal()])
                }
            })
            .await;

        let result = rx.await;
        assert_matches!(result, Ok(Ok(_)));
        let result = result.unwrap().unwrap();
        assert_eq!(result.len(), headers.len());
        assert_eq!(result, headers);
    }
}
