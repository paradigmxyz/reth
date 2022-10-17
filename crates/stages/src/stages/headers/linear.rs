use super::stage::{DownloadError, Downloader};
use async_trait::async_trait;
use rand::Rng;
use reth_interfaces::{
    consensus::Consensus,
    stages::{HeaderRequest, HeadersClient, MessageStream},
};
use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256};
use std::{sync::Arc, time::Duration};
use tokio_stream::StreamExt;

/// Download headers in batches
#[derive(Debug)]
pub struct LinearDownloader {
    /// Consensus client implementation
    pub consensus: Arc<dyn Consensus>,
    /// Downloader client implementation
    pub client: Arc<dyn HeadersClient>,
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: u64,
    /// The number of retries for downloading
    pub request_retries: usize,
}

#[async_trait]
impl Downloader for LinearDownloader {
    /// Download headers in batches with retries.
    /// Returns the header collection in sorted ascending order
    async fn download(
        &self,
        head: &HeaderLocked,
        tip: H256,
    ) -> Result<Vec<HeaderLocked>, DownloadError> {
        let mut stream = self.client.stream_headers().await;
        // Header order will be preserved during inserts
        let mut retries = self.request_retries;

        let mut out = Vec::<HeaderLocked>::new();
        loop {
            match self.download_batch(head, tip, &mut stream, &mut out).await {
                Ok(done) => {
                    if done {
                        return Ok(out)
                    }
                }
                Err(e) if e.is_retryable() && retries > 1 => {
                    retries -= 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl LinearDownloader {
    async fn download_batch(
        &self,
        head: &HeaderLocked,
        chain_tip: H256,
        stream: &mut MessageStream<(u64, Vec<Header>)>,
        out: &mut Vec<HeaderLocked>,
    ) -> Result<bool, DownloadError> {
        // Request headers starting from tip or earliest cached
        let start = out.first().map_or(chain_tip, |h| h.parent_hash);
        let request_id = self.request_headers(start).await;

        // Filter stream by request id and non empty headers content
        let stream = stream.filter(|(id, headers)| request_id == *id && !headers.is_empty());

        // Wrap the stream with a timeout
        let stream = stream.timeout(Duration::from_secs(self.request_timeout));

        // Unwrap the latest stream message which will be either
        // the msg with headers or timeout error
        let headers = {
            let mut h = match Box::pin(stream).try_next().await {
                Ok(Some((_, h))) => h,
                _ => return Err(DownloadError::NoHeaderResponse { request_id }),
            };
            h.sort_unstable_by_key(|h| h.number);
            h
        };

        // Iterate the headers in reverse
        out.reserve_exact(headers.len());
        let mut headers_rev = headers.into_iter().rev();
        while let Some(parent) = headers_rev.next() {
            let parent = parent.lock();

            if head.hash() == parent.hash() {
                // We've reached the target
                return Ok(true)
            }

            if let Some(tail_header) = out.first() {
                if !(parent.hash() == tail_header.parent_hash &&
                    parent.number + 1 == tail_header.number)
                {
                    // Cannot attach to the current buffer,
                    // discard this batch
                    return Ok(false)
                }

                self.consensus.validate_header(&tail_header, &parent).map_err(|e| {
                    DownloadError::HeaderValidation { hash: parent.hash(), details: e.to_string() }
                })?;
            } else if parent.hash() != chain_tip {
                // The buffer is empty and the first header
                // does not match the one we requested
                // discard this batch
                // TODO: penalize the peer?
                return Ok(false)
            }

            out.insert(0, parent);
        }

        Ok(false)
    }

    /// Perform a header request. Return the request ID
    async fn request_headers(&self, start: H256) -> u64 {
        let request_id = rand::thread_rng().gen();
        let request =
            HeaderRequest { start: BlockId::Hash(start), limit: self.batch_size, reverse: true };
        let _ = self.client.send_header_request(request_id, request).await;
        request_id
    }
}

#[cfg(test)]
mod tests {
    use super::{super::stage::tests::utils, DownloadError, Downloader, LinearDownloader};
    use assert_matches::assert_matches;
    use rand::{self, Rng};
    use reth_interfaces::stages::HeaderRequest;
    use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256};
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc, oneshot};
    use tokio_stream::{wrappers::ReceiverStream, StreamExt};

    #[tokio::test]
    async fn download_batch_timeout() {
        let (tx, rx) = oneshot::channel();
        let (req_tx, req_rx) = mpsc::channel(1);
        let (_res_tx, res_rx) = broadcast::channel(1);
        let (batch_size, request_retries, request_timeout) = (1, 1, 1);

        let downloader = LinearDownloader {
            consensus: Arc::new(utils::TestConsensus::new()),
            client: Arc::new(utils::TestHeaderClient::new(req_tx, res_rx)),
            batch_size,
            request_retries,
            request_timeout,
        };

        tokio::spawn(async move {
            let result = downloader.download(&HeaderLocked::default(), H256::zero()).await;
            tx.send(result).expect("failed to forward download response");
        });

        let requests = ReceiverStream::new(req_rx).collect::<Vec<_>>().await;
        assert_eq!(requests.len(), request_retries);
        assert_matches!(rx.await, Ok(Err(DownloadError::NoHeaderResponse { .. })));
    }

    #[tokio::test]
    async fn download_batch_timeout_on_invalid_messages() {
        let (tx, rx) = oneshot::channel();
        let (req_tx, req_rx) = mpsc::channel(1);
        let (res_tx, res_rx) = broadcast::channel(1);
        let (batch_size, request_retries, request_timeout) = (1, 5, 1);

        let client = Arc::new(utils::TestHeaderClient::new(req_tx, res_rx));
        let downloader = LinearDownloader {
            consensus: Arc::new(utils::TestConsensus::new()),
            client: client.clone(),
            batch_size,
            request_retries,
            request_timeout,
        };

        tokio::spawn(async move {
            let result = downloader.download(&HeaderLocked::default(), H256::zero()).await;
            tx.send(result).expect("failed to forward download response");
        });

        let mut num_of_reqs = 0;
        let mut last_req_id = None;
        let mut req_stream = Box::pin(ReceiverStream::new(req_rx));
        while let Some((id, _req)) = req_stream.next().await {
            // Since the receiving channel filters by id and message length -
            // randomize the input to the tested filter
            res_tx.send((id.saturating_add(id % 2), vec![])).expect("failed to send response");
            num_of_reqs += 1;
            last_req_id = Some(id);

            if num_of_reqs == request_retries {
                drop(res_tx);
                break
            }
        }

        assert_eq!(num_of_reqs, request_retries);
        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::NoHeaderResponse { request_id })) if request_id == last_req_id.unwrap()
        );
    }

    #[tokio::test]
    async fn download_batch_propagates_consensus_error() {
        let (tx, rx) = oneshot::channel();
        let (req_tx, req_rx) = mpsc::channel(1);
        let (res_tx, res_rx) = broadcast::channel(1);
        let (batch_size, request_retries, request_timeout) = (1, 3, 5);

        let mut head_block = Header::default();
        head_block.state_root = H256::from_low_u64_be(rand::thread_rng().gen());
        let chain_tip = head_block.hash_slow();

        let mut consensus = utils::TestConsensus::new();
        consensus.update_tip(chain_tip);
        consensus.set_fail_validation(false);

        let downloader = LinearDownloader {
            consensus: Arc::new(consensus),
            client: Arc::new(utils::TestHeaderClient::new(req_tx, res_rx)),
            batch_size,
            request_retries,
            request_timeout,
        };

        tokio::spawn(async move {
            let result = downloader.download(&HeaderLocked::default(), H256::zero()).await;
            tx.send(result).expect("failed to forward download response");
        });

        let request = ReceiverStream::new(req_rx).next().await;
        assert_matches!(
            request,
            Some((_, HeaderRequest { start, .. }))
                if matches!(start, BlockId::Hash(hash) if hash == chain_tip)
        );

        let request = request.unwrap();
        res_tx.send((request.0, vec![head_block])).expect("failed to send header");

        assert_matches!(
            rx.await,
            Ok(Err(DownloadError::HeaderValidation { hash, .. })) // TODO:
        );

        // TODO: match the propagated error
    }
}
