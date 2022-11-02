use crate::{
    util::unwind::{unwind_table_by_num, unwind_table_by_num_hash},
    DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput,
    UnwindOutput,
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    db::{
        models::blocks::BlockNumHash, tables, DBContainer, Database, DatabaseGAT, DbCursorRO,
        DbCursorRW, DbTx, DbTxMut,
    },
    p2p::headers::{
        client::HeadersClient,
        downloader::{DownloadError, Downloader},
    },
};
use reth_primitives::{rpc::BigEndianHash, BlockNumber, HeaderLocked, H256, U256};
use std::fmt::Debug;
use tracing::*;

const HEADERS: StageId = StageId("Headers");

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

#[async_trait::async_trait]
impl<DB: Database, D: Downloader, C: Consensus, H: HeadersClient> Stage<DB>
    for HeaderStage<D, C, H>
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        HEADERS
    }

    /// Download the headers in reverse order
    /// starting from the tip
    async fn execute(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let tx = db.get_mut();
        let last_block_num = input.stage_progress.unwrap_or_default();
        self.update_head::<DB>(tx, last_block_num).await?;

        // download the headers
        // TODO: handle input.max_block
        let last_hash = tx
            .get::<tables::CanonicalHeaders>(last_block_num)?
            .ok_or(DatabaseIntegrityError::NoCannonicalHash { number: last_block_num })?;
        let last_header =
            tx.get::<tables::Headers>((last_block_num, last_hash).into())?.ok_or({
                DatabaseIntegrityError::NoHeader { number: last_block_num, hash: last_hash }
            })?;
        let head = HeaderLocked::new(last_header, last_hash);

        let forkchoice = self.next_fork_choice_state(&head.hash()).await;
        // The stage relies on the downloader to return the headers
        // in descending order starting from the tip down to
        // the local head (latest block in db)
        let headers = match self.downloader.download(&head, &forkchoice).await {
            Ok(res) => {
                // TODO: validate the result order?
                // at least check if it attaches (first == tip && last == last_hash)
                res
            }
            Err(e) => match e {
                DownloadError::Timeout { request_id } => {
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
                // TODO: this error is never propagated, clean up
                DownloadError::MismatchedHeaders { .. } => {
                    return Err(StageError::Validation { block: last_block_num })
                }
            },
        };

        let stage_progress = self.write_headers::<DB>(tx, headers).await?.unwrap_or(last_block_num);
        Ok(ExecOutput { stage_progress, reached_tip: true, done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: handle bad block
        let tx = db.get_mut();
        unwind_table_by_num::<DB, tables::CanonicalHeaders>(tx, input.unwind_to)?;
        unwind_table_by_num_hash::<DB, tables::HeaderNumbers>(tx, input.unwind_to)?;
        unwind_table_by_num_hash::<DB, tables::Headers>(tx, input.unwind_to)?;
        unwind_table_by_num_hash::<DB, tables::HeaderTD>(tx, input.unwind_to)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

impl<D: Downloader, C: Consensus, H: HeadersClient> HeaderStage<D, C, H> {
    async fn update_head<DB: Database>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let hash = tx
            .get::<tables::CanonicalHeaders>(height)?
            .ok_or_else(|| DatabaseIntegrityError::NoCannonicalHeader { number: height })?;
        let td: Vec<u8> = tx.get::<tables::HeaderTD>((height, hash).into())?.unwrap(); // TODO:
        self.client.update_status(height, hash, H256::from_slice(&td)).await;
        Ok(())
    }

    async fn next_fork_choice_state(&self, head: &H256) -> ForkchoiceState {
        let mut state_rcv = self.consensus.fork_choice_state();
        loop {
            let _ = state_rcv.changed().await;
            let forkchoice = state_rcv.borrow();
            if !forkchoice.head_block_hash.is_zero() && forkchoice.head_block_hash != *head {
                return forkchoice.clone()
            }
        }
    }

    /// Write downloaded headers to the database
    async fn write_headers<DB: Database>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        headers: Vec<HeaderLocked>,
    ) -> Result<Option<BlockNumber>, StageError> {
        let mut cursor_header_number = tx.cursor_mut::<tables::HeaderNumbers>()?;
        let mut cursor_header = tx.cursor_mut::<tables::Headers>()?;
        let mut cursor_canonical = tx.cursor_mut::<tables::CanonicalHeaders>()?;
        let mut cursor_td = tx.cursor_mut::<tables::HeaderTD>()?;
        let mut td = U256::from_big_endian(&cursor_td.last()?.map(|(_, v)| v).unwrap());

        let mut latest = None;
        // Since the headers were returned in descending order,
        // iterate them in the reverse order
        for header in headers.into_iter().rev() {
            if header.number == 0 {
                continue
            }

            let key: BlockNumHash = (header.number, header.hash()).into();
            let header = header.unlock();
            latest = Some(header.number);

            td += header.difficulty;

            // TODO: investigate default write flags
            cursor_header_number.append(key, header.number)?;
            cursor_header.append(key, header)?;
            cursor_canonical.append(key.number(), key.hash())?;
            cursor_td.append(key, H256::from_uint(&td).as_bytes().to_vec())?;
        }

        Ok(latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_utils::StageTestRunner;
    use assert_matches::assert_matches;
    use once_cell::sync::Lazy;
    use reth_interfaces::test_utils::{
        gen_random_header, gen_random_header_range, TestConsensus, TestHeadersClient,
    };
    use test_utils::{HeadersTestRunner, TestDownloader};

    const TEST_STAGE: StageId = StageId("Headers");
    static CONSENSUS: Lazy<TestConsensus> = Lazy::new(|| TestConsensus::default());
    static CLIENT: Lazy<TestHeadersClient> = Lazy::new(|| TestHeadersClient::default());

    #[tokio::test]
    // Check that the execution errors on empty database or
    // prev progress missing from the database.
    async fn execute_empty_db() {
        let runner = HeadersTestRunner::default();
        let stage = create_stage(Ok(vec![]));
        let rx = runner.execute(stage, ExecInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::NoCannonicalHeader { .. }))
        );
    }

    #[tokio::test]
    // Check that the execution exits on downloader timeout.
    async fn execute_timeout() {
        let runner = HeadersTestRunner::default();
        let head = gen_random_header(0, None);
        runner.insert_header(&head).expect("failed to insert header");

        let stage = create_stage(Err(DownloadError::Timeout { request_id: 0 }));
        let rx = runner.execute(stage, ExecInput::default());
        CONSENSUS.update_tip(H256::from_low_u64_be(1));
        assert_matches!(rx.await.unwrap(), Ok(ExecOutput { done, .. }) if !done);
    }

    #[tokio::test]
    // Check that validation error is propagated during the execution.
    async fn execute_validation_error() {
        let runner = HeadersTestRunner::default();
        let head = gen_random_header(0, None);
        runner.insert_header(&head).expect("failed to insert header");

        let stage = create_stage(Err(DownloadError::HeaderValidation {
            hash: H256::zero(),
            details: "".to_owned(),
        }));
        let rx = runner.execute(stage, ExecInput::default());
        CONSENSUS.update_tip(H256::from_low_u64_be(1));

        assert_matches!(rx.await.unwrap(), Err(StageError::Validation { block }) if block == 0);
    }

    #[tokio::test]
    // Validate that all necessary tables are updated after the
    // header download on no previous progress.
    async fn execute_no_progress() {
        let runner = HeadersTestRunner::default();
        let (start, end) = (0, 100);
        let head = gen_random_header(start, None);
        runner.insert_header(&head).expect("failed to insert header");

        let headers = gen_random_header_range(start + 1..end, head.hash());
        let stage = create_stage(Ok(headers.iter().rev().cloned().collect::<Vec<_>>()));
        let rx = runner.execute(stage, ExecInput::default());
        let tip = headers.last().unwrap();
        CONSENSUS.update_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { .. }));
        let result = result.unwrap();
        assert!(result.done && result.reached_tip);
        assert_eq!(result.stage_progress, tip.number);

        for header in headers {
            assert!(runner.validate_db_header(&header).is_ok());
        }
    }

    #[tokio::test]
    // Validate that all necessary tables are updated after the
    // header download with some previous progress.
    async fn stage_prev_progress() {
        let runner = HeadersTestRunner::default();
        // TODO: set bigger range once `MDBX_EKEYMISMATCH` issue is resolved
        let (start, end) = (10000, 10240);
        let head = gen_random_header(start, None);
        let headers = gen_random_header_range(start + 1..end, head.hash());
        runner.insert_header(&head).expect("failed to insert header");

        let stage = create_stage(Ok(headers.iter().rev().cloned().collect::<Vec<_>>()));
        let input = ExecInput {
            previous_stage: Some((TEST_STAGE, head.number)),
            stage_progress: Some(head.number),
        };
        let rx = runner.execute(stage, input);
        let tip = headers.last().unwrap();
        CONSENSUS.update_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { .. }));
        let result = result.unwrap();
        assert!(result.done && result.reached_tip);
        assert_eq!(result.stage_progress, tip.number);

        for header in headers {
            assert!(runner.validate_db_header(&header).is_ok());
        }
    }

    #[tokio::test]
    // Check that unwind does not panic on empty database.
    async fn unwind_empty_db() {
        let runner = HeadersTestRunner::default();
        let input = UnwindInput { bad_block: None, stage_progress: 100, unwind_to: 100 };
        let rx = runner.unwind(create_stage(Ok(vec![])), input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput {stage_progress} ) if stage_progress == input.unwind_to
        );
    }

    #[tokio::test]
    // Check that unwind can remove headers across gaps
    async fn unwind_db_gaps() {
        let runner = HeadersTestRunner::default();
        let head = gen_random_header(0, None);
        let first_range = gen_random_header_range(1..20, head.hash());
        let second_range = gen_random_header_range(50..100, H256::zero());
        runner.insert_header(&head).expect("failed to insert header");
        runner
            .insert_headers(first_range.iter().chain(second_range.iter()))
            .expect("failed to insert headers");

        let stage = create_stage(Ok(vec![]));
        let input = UnwindInput { bad_block: None, stage_progress: 15, unwind_to: 15 };
        let rx = runner.unwind(stage, input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput {stage_progress} ) if stage_progress == input.unwind_to
        );

        runner
            .db()
            .check_no_entry_above::<tables::CanonicalHeaders, _>(input.unwind_to, |key| key)
            .expect("failed to check cannonical headers");
        runner
            .db()
            .check_no_entry_above::<tables::HeaderNumbers, _>(input.unwind_to, |key| key.number())
            .expect("failed to check header numbers");
        runner
            .db()
            .check_no_entry_above::<tables::Headers, _>(input.unwind_to, |key| key.number())
            .expect("failed to check headers");
        runner
            .db()
            .check_no_entry_above::<tables::HeaderTD, _>(input.unwind_to, |key| key.number())
            .expect("failed to check td");
    }

    // Create an instance of [HeaderStage]
    fn create_stage<'a>(
        result: Result<Vec<HeaderLocked>, DownloadError>,
    ) -> HeaderStage<TestDownloader, &'a TestConsensus, &'a TestHeadersClient> {
        HeaderStage {
            client: &*CLIENT,
            consensus: &*CONSENSUS,
            downloader: test_utils::TestDownloader::new(result),
        }
    }

    mod test_utils {
        use crate::util::test_utils::{StageTestDB, StageTestRunner};
        use async_trait::async_trait;
        use reth_interfaces::{
            consensus::ForkchoiceState,
            db::{self, models::blocks::BlockNumHash, tables, DbTx},
            p2p::headers::downloader::{DownloadError, Downloader},
            test_utils::{TestConsensus, TestHeadersClient},
        };
        use reth_primitives::{rpc::BigEndianHash, HeaderLocked, H256, U256};
        use std::{ops::Deref, time::Duration};

        #[derive(Default)]
        pub(crate) struct HeadersTestRunner {
            db: StageTestDB,
        }

        impl StageTestRunner for HeadersTestRunner {
            fn db(&self) -> &StageTestDB {
                &self.db
            }
        }

        impl HeadersTestRunner {
            /// Insert header into tables
            pub(crate) fn insert_header(&self, header: &HeaderLocked) -> Result<(), db::Error> {
                self.insert_headers(std::iter::once(header))
            }

            /// Insert headers into tables
            pub(crate) fn insert_headers<'a, I>(&self, headers: I) -> Result<(), db::Error>
            where
                I: Iterator<Item = &'a HeaderLocked>,
            {
                let headers = headers.collect::<Vec<_>>();
                self.db.map_put::<tables::HeaderNumbers, _, _>(&headers, |h| {
                    (BlockNumHash((h.number, h.hash())), h.number)
                })?;
                self.db.map_put::<tables::Headers, _, _>(&headers, |h| {
                    (BlockNumHash((h.number, h.hash())), h.deref().clone().unlock())
                })?;
                self.db.map_put::<tables::CanonicalHeaders, _, _>(&headers, |h| {
                    (h.number, h.hash())
                })?;

                self.db.transform_append::<tables::HeaderTD, _, _>(&headers, |prev, h| {
                    let prev_td = U256::from_big_endian(&prev.clone().unwrap_or_default());
                    (
                        BlockNumHash((h.number, h.hash())),
                        H256::from_uint(&(prev_td + h.difficulty)).as_bytes().to_vec(),
                    )
                })?;

                Ok(())
            }

            /// Validate stored header against provided
            pub(crate) fn validate_db_header(
                &self,
                header: &HeaderLocked,
            ) -> Result<(), db::Error> {
                let db = self.db.container();
                let tx = db.get();
                let key: BlockNumHash = (header.number, header.hash()).into();

                let db_number = tx.get::<tables::HeaderNumbers>(key)?;
                assert_eq!(db_number, Some(header.number));

                let db_header = tx.get::<tables::Headers>(key)?;
                assert_eq!(db_header, Some(header.clone().unlock()));

                let db_canonical_header = tx.get::<tables::CanonicalHeaders>(header.number)?;
                assert_eq!(db_canonical_header, Some(header.hash()));

                if header.number != 0 {
                    let parent_key: BlockNumHash = (header.number - 1, header.parent_hash).into();
                    let parent_td = tx.get::<tables::HeaderTD>(parent_key)?;
                    let td = U256::from_big_endian(&tx.get::<tables::HeaderTD>(key)?.unwrap());
                    assert_eq!(
                        parent_td.map(|td| U256::from_big_endian(&td) + header.difficulty),
                        Some(td)
                    );
                }

                Ok(())
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
            type Client = TestHeadersClient;

            fn timeout(&self) -> Duration {
                Duration::from_secs(1)
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
