use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use async_trait::async_trait;
use rand::Rng;
use reth_db::{
    kv::{table::Encode, tables, tx::Tx},
    mdbx::{self, WriteFlags},
};
use reth_interfaces::{
    consensus::Consensus,
    stages::{HeaderRequest, HeadersClient, MessageStream},
};
use reth_primitives::{rpc::BlockId, BigEndianHash, BlockNumber, Header, HeaderLocked, H256, U256};
use std::{sync::Arc, time::Duration};
use thiserror::Error;
use tokio_stream::StreamExt;
use tracing::*;

const HEADERS: StageId = StageId("HEADERS");

// TODO: docs
// TODO: add tracing

/// The headers stage implementation for staged sync
#[derive(Debug)]
pub struct HeaderStage {
    /// Consensus client implementation
    pub consensus: Arc<dyn Consensus>,
    /// Downloader client implementation
    pub client: Arc<dyn HeadersClient>,
    /// The batch size per one request
    pub batch_size: u64,
    /// A single request timeout
    pub request_timeout: u64,
    /// The number of retries for downloadign
    pub request_retries: usize,
}

/// The downloader error type
#[derive(Error, Debug)]
pub enum DownloadError {
    /// Header validation failed
    #[error("Failed to validate header {hash}. Details: {details}.")]
    HeaderValidation {
        /// Hash of header failing validation
        hash: H256,
        /// The details of validation failure
        details: String,
    },
    /// No headers reponse received
    #[error("Failed to get headers for request {request_id}.")]
    NoHeaderResponse {
        /// The last request ID
        request_id: u64,
    },
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl DownloadError {
    fn is_retryable(&self) -> bool {
        matches!(self, DownloadError::NoHeaderResponse { .. })
    }
}

#[async_trait]
impl<'db, E> Stage<'db, E> for HeaderStage
where
    E: mdbx::EnvironmentKind,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        HEADERS
    }

    /// Download the headers in reverse order
    /// starting from the tip
    async fn execute<'tx>(
        &mut self,
        tx: &mut Tx<'tx, mdbx::RW, E>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>
    where
        'db: 'tx,
    {
        let last_block_num =
            input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();
        // TODO: check if in case of panic the node head needs to be updated
        self.update_head(tx, last_block_num).await?;

        // download the headers
        // TODO: check if some upper block constraint is necessary
        let last_hash =
            H256::from_uint(&tx.get::<tables::CanonicalHeaders>(last_block_num)?.unwrap());
        let last_header: Header = temp::decode_header(
            tx.get::<tables::Headers>(temp::num_hash_to_key(last_block_num, last_hash))?.unwrap(),
        );
        let head = HeaderLocked::new(last_header, last_hash);

        let forkchoice_state = self.next_forkchoice_state(&head.hash()).await;

        let headers = match self.download(&head, forkchoice_state).await {
            Ok(res) => res,
            Err(e) => match e {
                DownloadError::NoHeaderResponse { request_id } => {
                    warn!("no response for header request {request_id}");
                    return Ok(ExecOutput {
                        stage_progress: last_block_num,
                        reached_tip: false,
                        done: false,
                    })
                }
                DownloadError::HeaderValidation { hash, details } => {
                    warn!("validation error for header {hash}: {details}");
                    return Err(StageError::Validation { block: last_block_num })
                }
                DownloadError::Internal(e) => return Err(StageError::Internal(e)),
            },
        };

        let stage_progress = self.write_headers(tx, headers).await?;
        Ok(ExecOutput { stage_progress, reached_tip: true, done: true })
    }

    /// Unwind the stage.
    async fn unwind<'tx>(
        &mut self,
        tx: &mut Tx<'tx, mdbx::RW, E>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(bad_block) = input.bad_block {
            todo!()
        }

        let mut walker = tx.cursor::<tables::CanonicalHeaders>()?.walk(input.unwind_to + 1)?;
        while let Some((_, hash)) = walker.next().transpose()? {
            tx.delete::<tables::HeaderNumbers>(hash.encode().to_vec(), None)?;
        }

        // TODO: cleanup
        let mut cur = tx.cursor::<tables::Headers>()?;
        let mut entry = cur.last()?;
        while let Some((key, _)) = entry {
            let (num, _) = temp::num_hash_from_key(key);
            if num <= input.unwind_to {
                break
            }
            cur.delete()?;
            entry = cur.prev()?;
        }

        let mut cur = tx.cursor::<tables::CanonicalHeaders>()?;
        let mut entry = cur.last()?;
        while let Some((block_num, _)) = entry {
            if block_num <= input.unwind_to {
                break
            }
            cur.delete()?;
            entry = cur.prev()?;
        }

        let mut cur = tx.cursor::<tables::HeaderTD>()?;
        let mut entry = cur.last()?;
        while let Some((key, _)) = entry {
            let (num, _) = temp::num_hash_from_key(key);
            if num <= input.unwind_to {
                break
            }
            cur.delete()?;
            entry = cur.prev()?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

impl HeaderStage {
    async fn update_head<'tx, E: mdbx::EnvironmentKind>(
        &self,
        tx: &'tx mut Tx<'_, mdbx::RW, E>,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let hash = H256::from_uint(&tx.get::<tables::CanonicalHeaders>(height)?.unwrap());
        let td: Vec<u8> = tx.get::<tables::HeaderTD>(temp::num_hash_to_key(height, hash))?.unwrap();
        self.client.update_status(height, hash, H256::from_slice(&td)).await;
        Ok(())
    }

    async fn next_forkchoice_state(&self, head: &H256) -> H256 {
        let mut state_rcv = self.consensus.forkchoice_state();
        loop {
            let _ = state_rcv.changed().await;
            let forkchoice = state_rcv.borrow();
            if !forkchoice.head_block_hash.is_zero() && forkchoice.head_block_hash != *head {
                return forkchoice.head_block_hash
            }
        }
    }

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
                Err(e) if e.is_retryable() && retries > 0 => {
                    retries -= 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

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

    /// Write downloaded headers to the database
    async fn write_headers<'tx, E: mdbx::EnvironmentKind>(
        &self,
        tx: &'tx mut Tx<'_, mdbx::RW, E>,
        headers: Vec<HeaderLocked>,
    ) -> Result<BlockNumber, StageError> {
        let mut cursor_header_number = tx.cursor::<tables::HeaderNumbers>()?;
        let mut cursor_header = tx.cursor::<tables::Headers>()?;
        let mut cursor_canonical = tx.cursor::<tables::CanonicalHeaders>()?;
        let mut cursor_td = tx.cursor::<tables::HeaderTD>()?;
        let mut td = U256::from_big_endian(&cursor_td.last()?.map(|(_, v)| v).unwrap());

        // TODO: comment
        let mut latest = 0;
        for header in headers {
            if header.number == 0 {
                continue
            }

            let hash = header.hash();
            let number = header.number;
            let num_hash_key = temp::num_hash_to_key(header.number, hash);

            td += header.difficulty;

            cursor_header_number.put(hash.to_fixed_bytes().to_vec(), header.number, None)?;
            cursor_header.put(
                num_hash_key.clone(),
                temp::encode_header(header.unlock()),
                Some(WriteFlags::APPEND),
            )?;
            cursor_canonical.put(number, hash.into_uint(), Some(WriteFlags::APPEND))?;
            cursor_td.put(
                num_hash_key,
                H256::from_uint(&td).as_bytes().to_vec(),
                Some(WriteFlags::APPEND),
            )?;

            latest = number;
        }

        Ok(latest)
    }
}

// TODO: remove
mod temp {
    use super::*;

    pub(crate) fn num_hash_to_key(number: BlockNumber, hash: H256) -> Vec<u8> {
        let mut key = number.to_be_bytes().to_vec();
        key.extend(hash.0);
        key
    }

    pub(crate) fn num_hash_from_key(key: Vec<u8>) -> (BlockNumber, H256) {
        todo!()
    }

    pub(crate) fn encode_header(_header: Header) -> Vec<u8> {
        todo!()
    }

    pub(crate) fn decode_header(_bytes: Vec<u8>) -> Header {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::{DownloadError, HeaderStage};
    use assert_matches::assert_matches;
    use reth_interfaces::stages::{HeaderRequest, MessageStream};
    use reth_primitives::{HeaderLocked, H256};
    use std::sync::Arc;
    use tokio::sync::mpsc::{channel, Sender};
    use tokio_stream::{pending, wrappers::ReceiverStream, StreamExt};

    fn setup_stage(
        tx: Sender<(u64, HeaderRequest)>,
        batch_size: u64,
        request_timeout: u64,
        request_retries: usize,
    ) -> HeaderStage {
        let client = utils::TestHeaderClient::new(tx);
        let consensus = utils::TestConsensus::new();
        HeaderStage {
            consensus: Arc::new(consensus),
            client: Arc::new(client),
            batch_size,
            request_retries,
            request_timeout,
        }
    }

    #[tokio::test]
    async fn download_batch_timeout() {
        let (tx, rx) = channel(1);
        let (req_tx, _req_rx) = channel(1);
        let (batch, timeout, retries) = (1, 1, 1);
        let stage = setup_stage(req_tx, batch, timeout, retries);

        let mut stream = Box::pin(pending()) as MessageStream<utils::HeaderResponse>;
        tokio::spawn(async move {
            let result = stage
                .download_batch(&HeaderLocked::default(), H256::zero(), &mut stream, &mut vec![])
                .await;
            tx.send(result).await.unwrap();
        });

        assert_matches!(
            *ReceiverStream::new(rx).collect::<Vec<Result<bool, DownloadError>>>().await,
            [Err(DownloadError::NoHeaderResponse { .. })]
        );
    }

    #[tokio::test]
    async fn download_batch_timeout_on_invalid_messages() {
        let (tx, rx) = channel(1);
        let (req_tx, req_rx) = channel(3);
        let (res_tx, res_rx) = channel(3);

        let (batch, timeout, retries) = (1, 5, 3);
        let stage = setup_stage(req_tx, batch, timeout, retries);

        let mut stream =
            Box::pin(ReceiverStream::new(res_rx)) as MessageStream<utils::HeaderResponse>;
        tokio::spawn(async move {
            let result = stage
                .download_batch(&HeaderLocked::default(), H256::zero(), &mut stream, &mut vec![])
                .await;
            tx.send(result).await.unwrap();
        });

        let mut last_req_id = None;
        let mut req_stream = ReceiverStream::new(req_rx);
        while let Some((id, _req)) = req_stream.next().await {
            // Since the receiving channel filters by id and message length -
            // randomize the input to the tested filter
            res_tx.send((id.saturating_add(id % 2), vec![])).await.unwrap();
            last_req_id = Some(id);
        }

        assert_matches!(
            *ReceiverStream::new(rx).collect::<Vec<Result<bool, DownloadError>>>().await,
            [Err(DownloadError::NoHeaderResponse { request_id })] if request_id == last_req_id.unwrap()
        );
    }

    mod utils {
        use async_trait::async_trait;
        use reth_interfaces::{
            consensus::{self, Consensus},
            stages::{HeaderRequest, HeadersClient, MessageStream},
        };
        use reth_primitives::{Header, H256, H512};
        use reth_rpc_types::engine::ForkchoiceState;
        use std::collections::HashSet;
        use tokio::sync::{mpsc::Sender, watch};

        pub(crate) type HeaderResponse = (u64, Vec<Header>);

        #[derive(Debug)]
        pub(crate) struct TestHeaderClient {
            tx: Sender<(u64, HeaderRequest)>,
        }

        impl TestHeaderClient {
            /// Construct a new test header downloader.
            /// `tx` is the
            pub(crate) fn new(tx: Sender<(u64, HeaderRequest)>) -> Self {
                Self { tx }
            }
        }

        #[async_trait]
        impl HeadersClient for TestHeaderClient {
            async fn update_status(&self, _height: u64, _hash: H256, _td: H256) {}

            async fn send_header_request(&self, id: u64, request: HeaderRequest) -> HashSet<H512> {
                self.tx.send((id, request)).await.unwrap();
                HashSet::default()
            }

            async fn stream_headers(&self) -> MessageStream<(u64, Vec<Header>)> {
                todo!()
            }
        }

        /// Consensus client impl for testing
        #[derive(Debug)]
        pub(crate) struct TestConsensus {
            chain_tip: H256,
        }

        impl TestConsensus {
            pub(crate) fn new() -> Self {
                Self { chain_tip: H256::zero() }
            }

            /// Set the chain tip
            pub(crate) fn set_chain_tip(&mut self, tip: H256) {
                self.chain_tip = tip;
            }
        }

        #[async_trait]
        impl Consensus for TestConsensus {
            fn forkchoice_state(&self) -> watch::Receiver<ForkchoiceState> {
                todo!()
            }

            fn tip(&self) -> H256 {
                self.chain_tip
            }

            fn validate_header(
                &self,
                _header: &Header,
                _parent: &Header,
            ) -> Result<(), consensus::Error> {
                Ok(())
            }
        }
    }
}
