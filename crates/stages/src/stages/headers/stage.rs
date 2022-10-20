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
pub struct HeaderStage<D: Downloader, C: Consensus, H: HeadersClient> {
    /// Strategy for downloading the headers
    pub downloader: D,
    /// Consensus client implementation
    pub consensus: C,
    /// Downloader client implementation
    pub client: H,
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
impl<'db, E, D: Downloader, C: Consensus, H: HeadersClient> Stage<'db, E> for HeaderStage<D, C, H>
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
        let headers = match self.downloader.download(&head, &forkchoice).await {
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

impl<D: Downloader, C: Consensus, H: HeadersClient> HeaderStage<D, C, H> {
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
    use once_cell::sync::Lazy;
    use reth_db::{
        kv::{test_utils as test_db_utils, EnvKind},
        mdbx,
    };
    use tokio::sync::mpsc;

    static CONSENSUS: Lazy<test_utils::TestConsensus> =
        Lazy::new(|| test_utils::TestConsensus::new());

    static CLIENT: Lazy<test_utils::TestHeaderClient> =
        Lazy::new(|| test_utils::TestHeaderClient::new());

    #[tokio::test]
    async fn headers_stage_empty_db() {
        let mut stage = HeaderStage {
            client: &*CLIENT,
            consensus: &*CONSENSUS,
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
        use std::{
            collections::HashSet,
            ops::Range,
            sync::{Arc, Mutex},
        };
        use tokio::sync::{broadcast, mpsc, watch};
        use tokio_stream::{wrappers::BroadcastStream, StreamExt};

        pub(crate) fn gen_block_range(rng: Range<u64>, head: H256) -> Vec<HeaderLocked> {
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
            let mut header = Header::default();
            header.number = number;
            header.nonce = rand::random();
            header.parent_hash = parent.unwrap_or_default();
            header.lock()
        }

        pub(crate) type HeaderResponse = (u64, Vec<Header>);

        #[derive(Debug)]
        pub(crate) struct TestHeaderClient {
            req_tx: mpsc::Sender<(u64, HeaderRequest)>,
            req_rx: Arc<Mutex<mpsc::Receiver<(u64, HeaderRequest)>>>,
            res_tx: broadcast::Sender<HeaderResponse>,
            res_rx: broadcast::Receiver<HeaderResponse>,
        }

        impl TestHeaderClient {
            /// Construct a new test header downloader.
            pub(crate) fn new() -> Self {
                let (req_tx, req_rx) = mpsc::channel(1);
                let (res_tx, res_rx) = broadcast::channel(1);
                Self { req_tx, req_rx: Arc::new(Mutex::new(req_rx)), res_tx, res_rx }
            }

            pub(crate) async fn on_header_request<T, F>(&self, mut count: usize, mut f: F) -> Vec<T>
            where
                F: FnMut(u64, HeaderRequest) -> T,
            {
                let mut rx = self.req_rx.lock().unwrap();
                let mut results = vec![];
                while let Some((id, req)) = rx.recv().await {
                    results.push(f(id, req));
                    count -= 1;
                    if count == 0 {
                        break
                    }
                }
                return results
            }

            pub(crate) fn send_header_response(&self, id: u64, headers: Vec<Header>) {
                self.res_tx.send((id, headers)).expect("failed to send header response");
            }
        }

        #[async_trait]
        impl HeadersClient for TestHeaderClient {
            async fn update_status(&self, _height: u64, _hash: H256, _td: H256) {}

            async fn send_header_request(&self, id: u64, request: HeaderRequest) -> HashSet<H512> {
                self.req_tx.send((id, request)).await.expect("failed to send request");
                HashSet::default()
            }

            async fn stream_headers(&self) -> MessageStream<(u64, Vec<Header>)> {
                Box::pin(BroadcastStream::new(self.res_rx.resubscribe()).filter_map(|e| e.ok()))
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
            type Consensus = TestConsensus;
            type Client = TestHeaderClient;

            fn timeout(&self) -> u64 {
                1
            }

            fn consensus(&self) -> &Self::Consensus {
                unimplemented!()
            }

            fn client(&self) -> &Self::Client {
                unimplemented!()
            }

            async fn download(
                &self,
                _: &HeaderLocked,
                _: &ForkchoiceState,
            ) -> Result<Vec<HeaderLocked>, DownloadError> {
                self.result.clone()
            }
        }
    }
}
