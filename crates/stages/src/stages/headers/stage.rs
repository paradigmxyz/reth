use super::downloader::{DownloadError, Downloader};
use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use async_trait::async_trait;
use reth_db::{
    kv::{blocks::BlockNumHash, tables, tx::Tx},
    mdbx::{self, WriteFlags},
};
use reth_interfaces::{consensus::Consensus, stages::HeadersClient};
use reth_primitives::{BigEndianHash, BlockNumber, HeaderLocked, H256, U256};
use reth_rpc_types::engine::ForkchoiceState;
use std::{fmt::Debug, sync::Arc};
use thiserror::Error;
use tracing::*;

const HEADERS: StageId = StageId("HEADERS");

/// The headers stage implementation for staged sync
#[derive(Debug)]
pub struct HeaderStage<D: Downloader> {
    /// Strategy for downloading the headers
    pub downloader: D,
    /// Consensus client implementation
    pub consensus: Arc<dyn Consensus>,
    /// Downloader client implementation
    pub client: Arc<dyn HeadersClient>,
}

/// The header stage error
#[derive(Error, Debug)]
pub enum HeaderStageError {
    #[error("no cannonical hash for block #{number}")]
    NoCannonicalHash { number: BlockNumber },
    #[error("no cannonical hash for block #{number}")]
    NoCannonicalHeader { number: BlockNumber },
    #[error("no header for block #{number} ({hash})")]
    NoHeader { number: BlockNumber, hash: H256 },
}

impl Into<StageError> for HeaderStageError {
    fn into(self) -> StageError {
        StageError::Internal(anyhow::Error::new(self))
    }
}

#[async_trait]
impl<'db, E, D: Downloader> Stage<'db, E> for HeaderStage<D>
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
            tx.get::<tables::CanonicalHeaders>(last_block_num)?.ok_or_else(|| -> StageError {
                HeaderStageError::NoCannonicalHash { number: last_block_num }.into()
            })?;
        let last_header = tx
            .get::<tables::Headers>((last_block_num, last_hash).into())?
            .ok_or_else(|| -> StageError {
                HeaderStageError::NoHeader { number: last_block_num, hash: last_hash }.into()
            })?;
        let head = HeaderLocked::new(last_header, last_hash);

        let forkchoice = self.next_forkchoice_state(&head.hash()).await;
        let headers = match self
            .downloader
            .download(self.client.clone(), self.consensus.clone(), &head, &forkchoice)
            .await
        {
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
    ) -> Result<UnwindOutput, anyhow::Error> {
        if let Some(bad_block) = input.bad_block {
            todo!()
        }

        let mut walker = tx.cursor::<tables::CanonicalHeaders>()?.walk(input.unwind_to + 1)?;
        while let Some(key) = walker.next().transpose()? {
            tx.delete::<tables::HeaderNumbers>(key.into(), None)?;
        }

        // TODO: cleanup
        let mut cur = tx.cursor::<tables::Headers>()?;
        let mut entry = cur.last()?;
        while let Some((BlockNumHash((num, _)), _)) = entry {
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
        while let Some((BlockNumHash((num, _)), _)) = entry {
            if num <= input.unwind_to {
                break
            }
            cur.delete()?;
            entry = cur.prev()?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

impl<D: Downloader> HeaderStage<D> {
    async fn update_head<'tx, E: mdbx::EnvironmentKind>(
        &self,
        tx: &'tx mut Tx<'_, mdbx::RW, E>,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let hash = tx.get::<tables::CanonicalHeaders>(height)?.ok_or_else(|| -> StageError {
            HeaderStageError::NoCannonicalHeader { number: height }.into()
        })?;
        let td: Vec<u8> = tx.get::<tables::HeaderTD>((height, hash).into())?.unwrap(); // TODO:
        self.client.update_status(height, hash, H256::from_slice(&td)).await;
        Ok(())
    }

    async fn next_forkchoice_state(&self, head: &H256) -> ForkchoiceState {
        let mut state_rcv = self.consensus.forkchoice_state();
        loop {
            let _ = state_rcv.changed().await;
            let forkchoice = state_rcv.borrow();
            if !forkchoice.head_block_hash.is_zero() && forkchoice.head_block_hash != *head {
                return forkchoice.clone()
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

            let key: BlockNumHash = (header.number, header.hash()).into();
            let header = header.unlock();
            latest = header.number;

            td += header.difficulty;

            cursor_header_number.put(key, header.number, None)?;
            cursor_header.put(key, header, Some(WriteFlags::APPEND))?;
            cursor_canonical.put(key.0 .0, key.0 .1, Some(WriteFlags::APPEND))?;
            cursor_td.put(
                key,
                H256::from_uint(&td).as_bytes().to_vec(), // TODO:
                Some(WriteFlags::APPEND),
            )?;
        }

        Ok(latest)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::util::db::TxContainer;
    use assert_matches::assert_matches;
    use reth_db::{
        kv::{test_utils as test_db_utils, EnvKind},
        mdbx,
    };
    use tokio::sync::{broadcast, mpsc};

    #[tokio::test]
    async fn headers_stage_empty_db() {
        let (req_tx, _req_rx) = mpsc::channel(1);
        let (_res_tx, res_rx) = broadcast::channel(1);

        let mut stage = HeaderStage {
            client: Arc::new(test_utils::TestHeaderClient::new(req_tx, res_rx)),
            consensus: Arc::new(test_utils::TestConsensus::new()),
            downloader: test_utils::TestDownloader::new(Ok(vec![])),
        };

        let mut db = test_db_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);
        let mut tx = TxContainer::new(&mut db).unwrap();

        let input = ExecInput { previous_stage: None, stage_progress: None };
        assert_matches!(
            stage.execute(tx.get_mut(), input).await,
            Err(StageError::Internal(err))
                if matches!(
                    err.downcast_ref::<HeaderStageError>(),
                    Some(HeaderStageError::NoCannonicalHeader { .. }
                )
            )
        );
    }

    #[tokio::test]
    // TODO:
    async fn headers_stage_() {
        let (req_tx, _req_rx) = mpsc::channel(1);
        let (_res_tx, res_rx) = broadcast::channel(1);

        let mut stage = HeaderStage {
            client: Arc::new(test_utils::TestHeaderClient::new(req_tx, res_rx)),
            consensus: Arc::new(test_utils::TestConsensus::new()),
            downloader: test_utils::TestDownloader::new(Ok(vec![])),
        };

        let mut db = test_db_utils::create_test_db::<mdbx::WriteMap>(EnvKind::RW);
        let mut tx = TxContainer::new(&mut db).unwrap();

        let input = ExecInput { previous_stage: None, stage_progress: None };
        assert_matches!(
            stage.execute(tx.get_mut(), input).await,
            Err(StageError::Internal(err))
                if matches!(
                    err.downcast_ref::<HeaderStageError>(),
                    Some(HeaderStageError::NoCannonicalHeader { .. }
                )
            )
        );
    }

    pub(crate) mod test_utils {
        use super::super::{DownloadError, Downloader};
        use async_trait::async_trait;
        use reth_interfaces::{
            consensus::{self, Consensus},
            stages::{HeaderRequest, HeadersClient, MessageStream},
        };
        use reth_primitives::{Header, HeaderLocked, H256, H512};
        use reth_rpc_types::engine::ForkchoiceState;
        use std::{collections::HashSet, sync::Arc};
        use tokio::sync::{broadcast, mpsc::Sender, watch};
        use tokio_stream::{wrappers::BroadcastStream, StreamExt};

        pub(crate) type HeaderResponse = (u64, Vec<Header>);

        #[derive(Debug)]
        pub(crate) struct TestHeaderClient {
            tx: Sender<(u64, HeaderRequest)>,
            rx: broadcast::Receiver<HeaderResponse>,
        }

        impl TestHeaderClient {
            /// Construct a new test header downloader.
            /// `tx` is the `Sender` for header requests
            /// `rx` is the `Receiver` of header responses
            pub(crate) fn new(
                tx: Sender<(u64, HeaderRequest)>,
                rx: broadcast::Receiver<HeaderResponse>,
            ) -> Self {
                Self { tx, rx }
            }
        }

        #[async_trait]
        impl HeadersClient for TestHeaderClient {
            async fn update_status(&self, _height: u64, _hash: H256, _td: H256) {}

            async fn send_header_request(&self, id: u64, request: HeaderRequest) -> HashSet<H512> {
                self.tx.send((id, request)).await.expect("failed to send request");
                HashSet::default()
            }

            async fn stream_headers(&self) -> MessageStream<(u64, Vec<Header>)> {
                Box::pin(BroadcastStream::new(self.rx.resubscribe()).filter_map(|e| e.ok()))
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

        #[derive(Debug)]
        pub(crate) struct TestDownloader {
            result: Result<Vec<HeaderLocked>, DownloadError>,
        }

        impl TestDownloader {
            pub(crate) fn new(result: Result<Vec<HeaderLocked>, DownloadError>) -> Self {
                Self { result }
            }
        }

        #[async_trait]
        impl Downloader for TestDownloader {
            fn timeout(&self) -> u64 {
                1
            }

            async fn download(
                &self,
                _: Arc<dyn HeadersClient>,
                _: Arc<dyn Consensus>,
                _: &HeaderLocked,
                _: &ForkchoiceState,
            ) -> Result<Vec<HeaderLocked>, DownloadError> {
                self.result.clone()
            }
        }
    }
}
