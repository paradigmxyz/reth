use super::downloader::{DownloadError, Downloader};
use async_trait::async_trait;
use reth_interfaces::{
    consensus::Consensus,
    stages::{HeadersClient, MessageStream},
};
use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256};
use reth_rpc_types::engine::ForkchoiceState;
use std::sync::Arc;

/// TODO:
#[derive(Debug)]
pub struct ParallelDownloader<'a, C> {
    pub consensus: &'a C,
    /// The number of parallel requests
    pub par_count: usize,
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: u64,
    /// The number of retries for downloading
    pub request_retries: usize,
}

#[async_trait]
impl<'a, C: Consensus> Downloader for ParallelDownloader<'a, C> {
    type Consensus = C;

    /// The request timeout
    fn timeout(&self) -> u64 {
        self.request_timeout
    }

    fn consensus(&self) -> &Self::Consensus {
        self.consensus
    }

    /// Download the headers
    async fn download(
        &self,
        client: Arc<dyn HeadersClient>,
        head: &HeaderLocked,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<HeaderLocked>, DownloadError> {
        let mut stream = client.stream_headers().await;
        let mut retries = self.request_retries;
        let mut reached_finalized = false;
        todo!()
        // // Header order will be preserved during inserts
        // let mut out = Vec::<HeaderLocked>::new();

        // // Request blocks by hash until finalized hash
        // loop {
        //     let result = self
        //         .download_batch(
        //             client.clone(),
        //             consensus.clone(),
        //             head,
        //             forkchoice,
        //             &mut stream,
        //             &mut out,
        //         )
        //         .await;
        //     match result {
        //         Ok(result) => match result {
        //             ParallelResult::Discard | ParallelResult::Continue => (),
        //             ParallelResult::ReachedFinalized => reached_finalized = true,
        //             ParallelResult::ReachedHead => return Ok(out),
        //         },
        //         Err(e) if e.is_retryable() && retries > 1 => {
        //             retries -= 1;
        //         }
        //         Err(e) => return Err(e),
        //     }
        // }
    }
}

enum ParallelResult {
    Continue,
    Discard,
    ReachedFinalized,
    ReachedHead,
}

impl<'a, C: Consensus> ParallelDownloader<'a, C> {
    async fn download_batch(
        &'a self,
        client: Arc<dyn HeadersClient>,
        head: &'a HeaderLocked,
        forkchoice: &'a ForkchoiceState,
        stream: &'a mut MessageStream<(u64, Vec<Header>)>,
        out: &'a mut Vec<HeaderLocked>,
    ) -> Result<ParallelResult, DownloadError> {
        // Request headers starting from tip or earliest cached
        let start = out.first().map_or(forkchoice.head_block_hash, |h| h.parent_hash);
        let mut headers = self
            .download_headers(stream, client.clone(), BlockId::Hash(start), self.batch_size)
            .await?;
        headers.sort_unstable_by_key(|h| h.number);

        // Iterate the headers in reverse
        out.reserve_exact(headers.len());
        let mut headers_rev = headers.into_iter().rev();

        let mut result = ParallelResult::Continue;
        while let Some(parent) = headers_rev.next() {
            let parent = parent.lock();

            if parent.hash() == head.hash() {
                // We've reached the target
                return Ok(ParallelResult::ReachedHead)
            }

            match out.first() {
                Some(tail_header) if !self.validate(tail_header, &parent)? => {
                    // Cannot attach to the current buffer, discard
                    return Ok(ParallelResult::Discard)
                }
                // The buffer is empty and the first header does not match the tip, discard
                // TODO: penalize the peer?
                None if parent.hash() != forkchoice.head_block_hash => {
                    return Ok(ParallelResult::Discard)
                }
                _ => (),
            };

            if parent.hash() == forkchoice.finalized_block_hash {
                result = ParallelResult::ReachedFinalized;
            }

            out.insert(0, parent);
        }

        Ok(result)
    }
}
