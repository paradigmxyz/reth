use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    db::{
        self, models::blocks::BlockNumHash, tables, DBContainer, Database, DatabaseGAT, DbCursorRO,
        DbCursorRW, DbTx, DbTxMut, Table,
    },
    p2p::headers::{
        client::HeadersClient,
        downloader::{DownloadError, Downloader},
    },
};
use reth_primitives::{rpc::BigEndianHash, BlockNumber, HeaderLocked, H256, U256};
use std::fmt::Debug;
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
    /// Cannonical hash is missing from db
    #[error("no cannonical hash for block #{number}")]
    NoCannonicalHash {
        /// The block number key
        number: BlockNumber,
    },
    /// Cannonical header is missing from db
    #[error("no cannonical hash for block #{number}")]
    NoCannonicalHeader {
        /// The block number key
        number: BlockNumber,
    },
    /// Header is missing from db
    #[error("no header for block #{number} ({hash})")]
    NoHeader {
        /// The block number key
        number: BlockNumber,
        /// The block hash key
        hash: H256,
    },
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
        let last_block_num =
            input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();
        self.update_head::<DB>(tx, last_block_num).await?;

        // download the headers
        // TODO: handle input.max_block
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
        let tx = &mut db.get_mut();
        self.unwind_table::<DB, tables::CanonicalHeaders, _>(tx, input.unwind_to, |num| num)?;
        self.unwind_table::<DB, tables::HeaderNumbers, _>(tx, input.unwind_to, |key| key.0 .0)?;
        self.unwind_table::<DB, tables::Headers, _>(tx, input.unwind_to, |key| key.0 .0)?;
        self.unwind_table::<DB, tables::HeaderTD, _>(tx, input.unwind_to, |key| key.0 .0)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

impl<D: Downloader, C: Consensus, H: HeadersClient> HeaderStage<D, C, H> {
    async fn update_head<DB: Database>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let hash = tx.get::<tables::CanonicalHeaders>(height)?.ok_or_else(|| -> StageError {
            HeaderStageError::NoCannonicalHeader { number: height }.into()
        })?;
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
            cursor_canonical.append(key.0 .0, key.0 .1)?;
            cursor_td.append(key, H256::from_uint(&td).as_bytes().to_vec())?;
        }

        Ok(latest)
    }

    /// Unwind the table to a provided block
    fn unwind_table<DB, T, F>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        block: BlockNumber,
        mut selector: F,
    ) -> Result<(), db::Error>
    where
        DB: Database,
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        let mut cursor = tx.cursor_mut::<T>()?;
        let mut entry = cursor.last()?;
        while let Some((key, _)) = entry {
            if selector(key) <= block {
                break
            }
            cursor.delete_current()?;
            entry = cursor.prev()?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use once_cell::sync::Lazy;
    use reth_db::{kv::Env, mdbx::WriteMap};
    use reth_interfaces::{
        db::DBContainer,
        test_utils::{
            gen_random_header, gen_random_header_range, TestConsensus, TestHeadersClient,
        },
    };
    use std::{borrow::Borrow, sync::Arc};
    use test_utils::HeadersDB;
    use tokio::sync::oneshot;

    const TEST_STAGE: StageId = StageId("HEADERS");
    static CONSENSUS: Lazy<TestConsensus> = Lazy::new(|| TestConsensus::default());
    static CLIENT: Lazy<TestHeadersClient> = Lazy::new(|| TestHeadersClient::default());

    #[tokio::test]
    // Check that the execution errors on empty database or
    // prev progress missing from the database.
    async fn headers_execute_empty_db() {
        let db = HeadersDB::default();
        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = execute_stage(db.inner(), input, Ok(vec![]));
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::HeadersStage(HeaderStageError::NoCannonicalHeader { .. }))
        );
    }

    #[tokio::test]
    // Check that the execution exits on downloader timeout.
    async fn headers_execute_timeout() {
        let head = gen_random_header(0, None);
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");

        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = execute_stage(db.inner(), input, Err(DownloadError::Timeout { request_id: 0 }));
        CONSENSUS.update_tip(H256::from_low_u64_be(1));
        assert_matches!(rx.await.unwrap(), Ok(ExecOutput { done, .. }) if !done);
    }

    #[tokio::test]
    // Check that validation error is propagated during the execution.
    async fn headers_execute_validation_error() {
        let head = gen_random_header(0, None);
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");

        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = execute_stage(
            db.inner(),
            input,
            Err(DownloadError::HeaderValidation { hash: H256::zero(), details: "".to_owned() }),
        );
        CONSENSUS.update_tip(H256::from_low_u64_be(1));

        assert_matches!(rx.await.unwrap(), Err(StageError::Validation { block }) if block == 0);
    }

    #[tokio::test]
    // Validate that all necessary tables are updated after the
    // header download on no previous progress.
    async fn headers_execute_no_progress() {
        let (start, end) = (0, 100);
        let head = gen_random_header(start, None);
        let headers = gen_random_header_range(start + 1..end, head.hash());
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");

        let result: Vec<_> = headers.iter().rev().cloned().collect();
        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = execute_stage(db.inner(), input, Ok(result));
        let tip = headers.last().unwrap();
        CONSENSUS.update_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { .. }));
        let result = result.unwrap();
        assert!(result.done && result.reached_tip);
        assert_eq!(result.stage_progress, tip.number);

        for header in headers {
            assert!(db.validate_db_header(&header).is_ok());
        }
    }

    #[tokio::test]
    // Validate that all necessary tables are updated after the
    // header download with some previous progress.
    async fn headers_stage_prev_progress() {
        // TODO: set bigger range once `MDBX_EKEYMISMATCH` issue is resolved
        let (start, end) = (10000, 10240);
        let head = gen_random_header(start, None);
        let headers = gen_random_header_range(start + 1..end, head.hash());
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");

        let result: Vec<_> = headers.iter().rev().cloned().collect();
        let input = ExecInput {
            previous_stage: Some((TEST_STAGE, head.number)),
            stage_progress: Some(head.number),
        };
        let rx = execute_stage(db.inner(), input, Ok(result));
        let tip = headers.last().unwrap();
        CONSENSUS.update_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { .. }));
        let result = result.unwrap();
        assert!(result.done && result.reached_tip);
        assert_eq!(result.stage_progress, tip.number);

        for header in headers {
            assert!(db.validate_db_header(&header).is_ok());
        }
    }

    #[tokio::test]
    // Check that unwind does not panic on empty database.
    async fn headers_unwind_empty_db() {
        let db = HeadersDB::default();
        let input = UnwindInput { bad_block: None, stage_progress: 100, unwind_to: 100 };
        let rx = unwind_stage(db.inner(), input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput {stage_progress} ) if stage_progress == input.unwind_to
        );
    }

    #[tokio::test]
    // Check that unwind can remove headers across gaps
    async fn headers_unwind_db_gaps() {
        let head = gen_random_header(0, None);
        let first_range = gen_random_header_range(1..20, head.hash());
        let second_range = gen_random_header_range(50..100, H256::zero());
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");
        for header in first_range.iter().chain(second_range.iter()) {
            db.insert_header(&header).expect("failed to insert header");
        }

        let input = UnwindInput { bad_block: None, stage_progress: 15, unwind_to: 15 };
        let rx = unwind_stage(db.inner(), input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput {stage_progress} ) if stage_progress == input.unwind_to
        );

        db.check_no_entry_above::<tables::CanonicalHeaders, _>(input.unwind_to, |key| key)
            .expect("failed to check cannonical headers");
        db.check_no_entry_above::<tables::HeaderNumbers, _>(input.unwind_to, |key| key.0 .0)
            .expect("failed to check header numbers");
        db.check_no_entry_above::<tables::Headers, _>(input.unwind_to, |key| key.0 .0)
            .expect("failed to check headers");
        db.check_no_entry_above::<tables::HeaderTD, _>(input.unwind_to, |key| key.0 .0)
            .expect("failed to check td");
    }

    // A helper function to run [HeaderStage::execute]
    fn execute_stage(
        db: Arc<Env<WriteMap>>,
        input: ExecInput,
        download_result: Result<Vec<HeaderLocked>, DownloadError>,
    ) -> oneshot::Receiver<Result<ExecOutput, StageError>> {
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let db = db.clone();
            let mut db = DBContainer::<Env<WriteMap>>::new(db.borrow()).unwrap();
            let mut stage = HeaderStage {
                client: &*CLIENT,
                consensus: &*CONSENSUS,
                downloader: test_utils::TestDownloader::new(download_result),
            };
            let result = stage.execute(&mut db, input).await;
            db.commit().expect("failed to commit");
            tx.send(result).expect("failed to send result");
        });
        rx
    }

    // A helper function to run [HeaderStage::unwind]
    fn unwind_stage(
        db: Arc<Env<WriteMap>>,
        input: UnwindInput,
    ) -> oneshot::Receiver<Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>>> {
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let db = db.clone();
            let mut db = DBContainer::<Env<WriteMap>>::new(db.borrow()).unwrap();
            let mut stage = HeaderStage {
                client: &*CLIENT,
                consensus: &*CONSENSUS,
                downloader: test_utils::TestDownloader::new(Ok(vec![])),
            };
            let result = stage.unwind(&mut db, input).await;
            db.commit().expect("failed to commit");
            tx.send(result).expect("failed to send result");
        });
        rx
    }

    pub(crate) mod test_utils {
        use async_trait::async_trait;
        use reth_db::{
            kv::{test_utils::create_test_db, Env, EnvKind},
            mdbx,
            mdbx::WriteMap,
        };
        use reth_interfaces::{
            consensus::ForkchoiceState,
            db::{
                self, models::blocks::BlockNumHash, tables, DBContainer, DbCursorRO, DbCursorRW,
                DbTx, DbTxMut, Table,
            },
            p2p::headers::downloader::{DownloadError, Downloader},
            test_utils::{TestConsensus, TestHeadersClient},
        };
        use reth_primitives::{rpc::BigEndianHash, BlockNumber, HeaderLocked, H256, U256};
        use std::{borrow::Borrow, sync::Arc, time::Duration};

        pub(crate) struct HeadersDB {
            db: Arc<Env<WriteMap>>,
        }

        impl Default for HeadersDB {
            fn default() -> Self {
                Self { db: Arc::new(create_test_db::<mdbx::WriteMap>(EnvKind::RW)) }
            }
        }

        impl HeadersDB {
            pub(crate) fn inner(&self) -> Arc<Env<WriteMap>> {
                self.db.clone()
            }

            fn container(&self) -> DBContainer<'_, Env<WriteMap>> {
                DBContainer::new(self.db.borrow()).expect("failed to create db container")
            }

            /// Insert header into tables
            pub(crate) fn insert_header(&self, header: &HeaderLocked) -> Result<(), db::Error> {
                let mut db = self.container();
                let tx = db.get_mut();

                let key: BlockNumHash = (header.number, header.hash()).into();
                tx.put::<tables::HeaderNumbers>(key, header.number)?;
                tx.put::<tables::Headers>(key, header.clone().unlock())?;
                tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;

                let mut cursor_td = tx.cursor_mut::<tables::HeaderTD>()?;
                let td =
                    U256::from_big_endian(&cursor_td.last()?.map(|(_, v)| v).unwrap_or(vec![]));
                cursor_td
                    .append(key, H256::from_uint(&(td + header.difficulty)).as_bytes().to_vec())?;

                db.commit()?;
                Ok(())
            }

            /// Validate stored header against provided
            pub(crate) fn validate_db_header(
                &self,
                header: &HeaderLocked,
            ) -> Result<(), db::Error> {
                let db = self.container();
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

            /// Check there there is no table entry above given block
            pub(crate) fn check_no_entry_above<T: Table, F>(
                &self,
                block: BlockNumber,
                mut selector: F,
            ) -> Result<(), db::Error>
            where
                T: Table,
                F: FnMut(T::Key) -> BlockNumber,
            {
                let db = self.container();
                let tx = db.get();

                let mut cursor = tx.cursor::<T>()?;
                if let Some((key, _)) = cursor.last()? {
                    assert!(selector(key) <= block);
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
