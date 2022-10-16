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
use std::{fmt::Debug, sync::Arc};
use thiserror::Error;
use tracing::*;

const HEADERS: StageId = StageId("HEADERS");

/// The headers stage implementation for staged sync
#[derive(Debug)]
pub struct HeaderStage {
    /// Strategy for downloading the headers
    pub downloader: Arc<dyn Downloader>,
    /// Consensus client implementation
    pub consensus: Arc<dyn Consensus>,
    /// Downloader client implementation
    pub client: Arc<dyn HeadersClient>,
}

/// The header downloading strategy
#[async_trait]
pub trait Downloader: Sync + Send + Debug {
    /// Download the headers
    async fn download(
        &self,
        latest: &HeaderLocked,
        tip: H256,
    ) -> Result<Vec<HeaderLocked>, DownloadError>;
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
    /// Returns bool indicating whether this error is retryable or fatal
    pub fn is_retryable(&self) -> bool {
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

        let headers = match self.downloader.download(&head, forkchoice_state).await {
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
pub(crate) mod tests {
    pub(crate) mod utils {
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
            /// Watcher over the forkchoice state
            channel: (watch::Sender<ForkchoiceState>, watch::Receiver<ForkchoiceState>),
            /// Flag whether the header validation should purposefully fail
            fail_validation: bool,
        }

        impl TestConsensus {
            pub(crate) fn new() -> Self {
                Self {
                    channel: watch::channel(ForkchoiceState {
                        head_block_hash: H256::zero(),
                        finalized_block_hash: H256::zero(),
                        safe_block_hash: H256::zero(),
                    }),
                    fail_validation: false,
                }
            }

            /// Update the forkchoice state
            pub(crate) fn update_tip(&mut self, tip: H256) {
                let state = ForkchoiceState {
                    head_block_hash: tip,
                    finalized_block_hash: H256::zero(),
                    safe_block_hash: H256::zero(),
                };
                self.channel.0.send(state).expect("updating forkchoice state failed");
            }

            /// Update the validation flag
            pub(crate) fn set_fail_validation(&mut self, val: bool) {
                self.fail_validation = val;
            }
        }

        #[async_trait]
        impl Consensus for TestConsensus {
            /// Return the watcher over the forkchoice state
            fn forkchoice_state(&self) -> watch::Receiver<ForkchoiceState> {
                self.channel.1.clone()
            }

            /// Retrieve the current chain tip
            fn tip(&self) -> H256 {
                self.channel.1.borrow().head_block_hash
            }

            /// Validate the header against its parent
            fn validate_header(
                &self,
                _header: &Header,
                _parent: &Header,
            ) -> Result<(), consensus::Error> {
                if self.fail_validation {
                    Err(consensus::Error::ConsensusError)
                } else {
                    Ok(())
                }
            }
        }
    }
}
