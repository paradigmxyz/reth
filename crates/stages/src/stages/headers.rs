use async_trait::async_trait;

use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use futures::StreamExt;
use rand::Rng;
use reth_db::{
    kv::{
        table::{Decode, Encode},
        tables,
        tx::Tx,
    },
    mdbx::{self, WriteFlags},
};
use reth_interfaces::{
    consensus::Consensus,
    stages::{HeaderRequest, HeadersClient, MessageStream},
};
use reth_primitives::{rpc::BlockId, BlockNumber, Header, HeaderLocked, H256};
use std::{sync::Arc, time::Duration};
use thiserror::Error;
use tracing::*;

const HEADERS: StageId = StageId("HEADERS");

// TODO: docs
// TODO: add tracing
pub struct HeaderStage {
    pub consensus: Arc<dyn Consensus>,
    pub client: Arc<dyn HeadersClient>,
    pub batch_size: u64,
    pub request_retries: usize,
    pub request_timeout: usize,
}

#[derive(Error, Debug)]
pub enum DownloadError {
    /// Header validation failed
    #[error("Failed to validate header {hash}. Details: {details}.")]
    HeaderValidation { hash: H256, details: String },
    /// No headers reponse received
    #[error("Failed to get headers for request {request_id}.")]
    NoHeaderResponse { request_id: u64 },
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
    fn id(&self) -> StageId {
        HEADERS
    }

    /// Execute the stage.
    async fn execute<'tx>(
        &mut self,
        tx: &mut Tx<'tx, mdbx::RW, E>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let last_block_num =
            input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();
        // TODO: check if in case of panic the node head needs to be updated
        self.update_head(tx, last_block_num).await?;

        let mut stage_progress = last_block_num;

        // download the headers
        // TODO: check if some upper block constraint is necessary
        let last_hash: H256 = tx.get::<tables::CanonicalHeaders>(last_block_num)?.unwrap(); // TODO:
        let last_header: Header = tx.get::<tables::Headers>((last_block_num, last_hash))?.unwrap(); // TODO:
        let head = HeaderLocked::new(last_header, last_hash);

        let forkchoice_state = self.next_forkchoice_state(&head.hash()).await;

        let headers = match self.download(&head, forkchoice_state).await {
            Ok(res) => res,
            Err(e) => match e {
                DownloadError::NoHeaderResponse { request_id } => {
                    warn!("no response for request {request_id}");
                    return Ok(ExecOutput { stage_progress, reached_tip: false, done: false })
                }
                DownloadError::HeaderValidation { hash, details } => {
                    warn!("validation error for header {hash}: {details}");
                    return Err(StageError::Validation { block: last_block_num })
                }
                DownloadError::Internal(e) => return Err(StageError::Internal(e)),
            },
        };

        let mut cursor_header_number = tx.cursor::<tables::HeaderNumbers>()?;
        let mut cursor_header = tx.cursor::<tables::Headers>()?;
        let mut cursor_canonical = tx.cursor::<tables::CanonicalHeaders>()?;
        let mut cursor_td = tx.cursor::<tables::HeaderTD>()?;
        let mut td = cursor_td.last()?.map(|((_, _), v)| v).unwrap(); // TODO:

        for header in headers {
            if header.number == 0 {
                continue
            }

            let hash = header.hash();
            td += header.difficulty;

            cursor_header_number.put(
                hash.to_fixed_bytes().to_vec(),
                header.number,
                Some(WriteFlags::APPEND),
            )?;
            cursor_header.put((header.number, hash), header, Some(WriteFlags::APPEND))?;
            cursor_canonical.put(header.number, hash, Some(WriteFlags::APPEND))?;
            cursor_td.put((header.number, hash), td, Some(WriteFlags::APPEND))?;

            stage_progress = header.number;
        }

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

        todo!()
    }
}

impl HeaderStage {
    async fn update_head<'tx, E: mdbx::EnvironmentKind>(
        &self,
        tx: &'tx mut Tx<'tx, mdbx::RW, E>,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let hash = tx.get::<tables::CanonicalHeaders>(height)?.unwrap().decode();
        let td: Vec<u8> = tx.get::<tables::HeaderTD>((height, hash))?.unwrap();
        self.client.update_status(height, hash, H256::from_slice(&td));
        Ok(())
    }

    async fn next_forkchoice_state(&self, head: &H256) -> (H256, H256) {
        let mut state_rcv = self.consensus.forkchoice_state();
        loop {
            state_rcv.changed().await;
            let forkchoice = state_rcv.borrow();
            if !forkchoice.head_block_hash.is_zero() && forkchoice.head_block_hash != *head {
                return (forkchoice.head_block_hash, forkchoice.finalized_block_hash)
            }
        }
    }

    /// Download headers in batches with retries.
    /// Returns the header collection in sorted ascending order
    async fn download(
        &self,
        head: &HeaderLocked,
        forkchoice_state: (H256, H256),
    ) -> Result<Vec<HeaderLocked>, DownloadError> {
        let mut stream = self.client.stream_headers().await;
        // the header order will be preserved during inserts
        let mut retries = self.request_retries;

        let mut out = Vec::<HeaderLocked>::new();
        loop {
            match self.download_batch(head, &forkchoice_state, &mut stream, &mut out).await {
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

    /// Request and process the batch of headers
    async fn download_batch(
        &self,
        head: &HeaderLocked,
        (state_tip, state_finalized): &(H256, H256),
        stream: &mut MessageStream<(u64, Vec<Header>)>,
        out: &mut Vec<HeaderLocked>,
    ) -> Result<bool, DownloadError> {
        let request_id = rand::thread_rng().gen();
        let start = BlockId::Hash(out.first().map_or(state_tip.clone(), |h| h.parent_hash));
        let request = HeaderRequest { start, limit: self.batch_size, reverse: true };
        // TODO: timeout
        let _ = self.client.send_header_request(request_id, request).await;

        let mut batch = self.receive_headers(stream, request_id).await?;

        out.reserve_exact(batch.len());
        batch.sort_unstable_by_key(|h| h.number); // TODO: revise: par_sort?

        let mut batch_iter = batch.into_iter().rev();
        while let Some(parent) = batch_iter.next() {
            let parent = parent.lock();

            if head.hash() == parent.hash() {
                // we are done
                return Ok(true)
            }

            if let Some(tail_header) = out.first() {
                if !(parent.hash() == tail_header.parent_hash &&
                    parent.number + 1 == tail_header.number)
                {
                    // cannot attach to the current buffer
                    // discard this batch
                    return Ok(false)
                }

                self.consensus.validate_header(&tail_header, &parent).map_err(|e| {
                    DownloadError::HeaderValidation { hash: parent.hash(), details: e.to_string() }
                })?;
            } else if parent.hash() != *state_tip {
                // the buffer is empty and the first header
                // does not match the one we requested
                // discard this batch
                return Ok(false)
            }

            out.insert(0, parent);
        }

        Ok(false)
    }

    /// Process header message stream and return the request by id.
    /// The messages with empty headers are ignored.
    async fn receive_headers(
        &self,
        stream: &mut MessageStream<(u64, Vec<Header>)>,
        request_id: u64,
    ) -> Result<Vec<Header>, DownloadError> {
        let timeout = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(timeout);
        let result = loop {
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        Some((id, headers)) if request_id == id && !headers.is_empty() => break Some(headers),
                        _ => (),
                    }
                }
                _ = &mut timeout => {
                    break None;
                }
            }
        };

        result.ok_or(DownloadError::NoHeaderResponse { request_id })
    }
}
