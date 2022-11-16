use crate::{
    util::unwind::{unwind_table_by_num, unwind_table_by_num_hash, unwind_table_by_walker},
    DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput,
    UnwindOutput,
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    db::{
        models::blocks::BlockNumHash, tables, DBContainer, Database, DatabaseGAT, DbCursorRO,
        DbCursorRW, DbTx, DbTxMut,
    },
    p2p::headers::{client::HeadersClient, downloader::HeaderDownloader, error::DownloadError},
};
use reth_primitives::{rpc::BigEndianHash, BlockNumber, SealedHeader, H256, U256};
use std::{fmt::Debug, sync::Arc};
use tracing::*;

const HEADERS: StageId = StageId("Headers");

/// The headers stage.
///
/// The headers stage downloads all block headers from the highest block in the local database to
/// the perceived highest block on the network.
///
/// The headers are processed and data is inserted into these tables:
///
/// - [`HeaderNumbers`][reth_interfaces::db::tables::HeaderNumbers]
/// - [`Headers`][reth_interfaces::db::tables::Headers]
/// - [`CanonicalHeaders`][reth_interfaces::db::tables::CanonicalHeaders]
/// - [`HeaderTD`][reth_interfaces::db::tables::HeaderTD]
#[derive(Debug)]
pub struct HeaderStage<D: HeaderDownloader, C: Consensus, H: HeadersClient> {
    /// Strategy for downloading the headers
    pub downloader: D,
    /// Consensus client implementation
    pub consensus: Arc<C>,
    /// Downloader client implementation
    pub client: Arc<H>,
}

#[async_trait::async_trait]
impl<DB: Database, D: HeaderDownloader, C: Consensus, H: HeadersClient> Stage<DB>
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
            .ok_or(DatabaseIntegrityError::CanonicalHash { number: last_block_num })?;
        let last_header =
            tx.get::<tables::Headers>((last_block_num, last_hash).into())?.ok_or({
                DatabaseIntegrityError::Header { number: last_block_num, hash: last_hash }
            })?;
        let head = SealedHeader::new(last_header, last_hash);

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
                DownloadError::HeaderValidation { hash, error } => {
                    warn!("Validation error for header {hash}: {error}");
                    return Err(StageError::Validation { block: last_block_num, error })
                }
                // TODO: this error is never propagated, clean up
                // DownloadError::MismatchedHeaders { .. } => {
                //     return Err(StageError::Validation { block: last_block_num })
                // }
                _ => unreachable!(),
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

        unwind_table_by_walker::<DB, tables::CanonicalHeaders, tables::HeaderNumbers>(
            tx,
            input.unwind_to + 1,
        )?;

        unwind_table_by_num::<DB, tables::CanonicalHeaders>(tx, input.unwind_to)?;
        unwind_table_by_num_hash::<DB, tables::Headers>(tx, input.unwind_to)?;
        unwind_table_by_num_hash::<DB, tables::HeaderTD>(tx, input.unwind_to)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

impl<D: HeaderDownloader, C: Consensus, H: HeadersClient> HeaderStage<D, C, H> {
    async fn update_head<DB: Database>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let hash = tx
            .get::<tables::CanonicalHeaders>(height)?
            .ok_or(DatabaseIntegrityError::CanonicalHeader { number: height })?;
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
        headers: Vec<SealedHeader>,
    ) -> Result<Option<BlockNumber>, StageError> {
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

            let block_hash = header.hash();
            let key: BlockNumHash = (header.number, block_hash).into();
            let header = header.unseal();
            latest = Some(header.number);

            td += header.difficulty;

            // TODO: investigate default write flags
            // NOTE: HeaderNumbers are not sorted and can't be inserted with cursor.
            tx.put::<tables::HeaderNumbers>(block_hash, header.number)?;
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
    use crate::util::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, UnwindStageTestRunner, PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use test_runner::HeadersTestRunner;

    stage_test_suite!(HeadersTestRunner);

    // TODO: test consensus propagation error

    /// Check that the execution errors on empty database or
    /// prev progress missing from the database.
    #[tokio::test]
    // Validate that the execution does not fail on timeout
    async fn execute_timeout() {
        let mut runner = HeadersTestRunner::default();
        let input = ExecInput::default();
        runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);
        runner.consensus.update_tip(H256::from_low_u64_be(1));
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { done: false, reached_tip: false, stage_progress: 0 })
        );
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }

    /// Execute the stage with linear downloader
    #[tokio::test]
    async fn execute_with_linear_downloader() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        let (stage_progress, previous_stage) = (1000, 1200);
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        // skip `after_execution` hook for linear downloader
        let tip = headers.last().unwrap();
        runner.consensus.update_tip(tip.hash());

        let download_result = headers.clone();
        runner
            .client
            .on_header_request(1, |id, _| {
                let response = download_result.iter().map(|h| h.clone().unseal()).collect();
                runner.client.send_header_response(id, response)
            })
            .await;

        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { done: true, reached_tip: true, stage_progress }) if stage_progress == tip.number
        );
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }

    mod test_runner {
        use crate::{
            stages::headers::HeaderStage,
            util::test_utils::{
                ExecuteStageTestRunner, StageTestDB, StageTestRunner, TestRunnerError,
                UnwindStageTestRunner,
            },
            ExecInput, ExecOutput, UnwindInput,
        };
        use reth_headers_downloaders::linear::{LinearDownloadBuilder, LinearDownloader};
        use reth_interfaces::{
            db::{self, models::blocks::BlockNumHash, tables, DbTx},
            p2p::headers::downloader::HeaderDownloader,
            test_utils::{
                generators::{random_header, random_header_range},
                TestConsensus, TestHeaderDownloader, TestHeadersClient,
            },
        };
        use reth_primitives::{rpc::BigEndianHash, SealedHeader, H256, U256};
        use std::{ops::Deref, sync::Arc};

        pub(crate) struct HeadersTestRunner<D: HeaderDownloader> {
            pub(crate) consensus: Arc<TestConsensus>,
            pub(crate) client: Arc<TestHeadersClient>,
            downloader: Arc<D>,
            db: StageTestDB,
        }

        impl Default for HeadersTestRunner<TestHeaderDownloader> {
            fn default() -> Self {
                let client = Arc::new(TestHeadersClient::default());
                Self {
                    client: client.clone(),
                    consensus: Arc::new(TestConsensus::default()),
                    downloader: Arc::new(TestHeaderDownloader::new(client)),
                    db: StageTestDB::default(),
                }
            }
        }

        impl<D: HeaderDownloader + 'static> StageTestRunner for HeadersTestRunner<D> {
            type S = HeaderStage<Arc<D>, TestConsensus, TestHeadersClient>;

            fn db(&self) -> &StageTestDB {
                &self.db
            }

            fn stage(&self) -> Self::S {
                HeaderStage {
                    consensus: self.consensus.clone(),
                    client: self.client.clone(),
                    downloader: self.downloader.clone(),
                }
            }
        }

        #[async_trait::async_trait]
        impl<D: HeaderDownloader + 'static> ExecuteStageTestRunner for HeadersTestRunner<D> {
            type Seed = Vec<SealedHeader>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let start = input.stage_progress.unwrap_or_default();
                let head = random_header(start, None);
                self.insert_header(&head)?;

                // use previous progress as seed size
                let end = input.previous_stage.map(|(_, num)| num).unwrap_or_default() + 1;

                if start + 1 >= end {
                    return Ok(Vec::default())
                }

                let mut headers = random_header_range(start + 1..end, head.hash());
                headers.insert(0, head);
                Ok(headers)
            }

            async fn after_execution(&self, headers: Self::Seed) -> Result<(), TestRunnerError> {
                let tip = if !headers.is_empty() {
                    headers.last().unwrap().hash()
                } else {
                    H256::from_low_u64_be(rand::random())
                };
                self.consensus.update_tip(tip);
                self.client
                    .send_header_response_delayed(
                        0,
                        headers.into_iter().map(|h| h.unseal()).collect(),
                        1,
                    )
                    .await;
                Ok(())
            }

            fn validate_execution(
                &self,
                _input: ExecInput,
                _output: Option<ExecOutput>,
            ) -> Result<(), TestRunnerError> {
                // TODO: refine
                // if let Some(ref headers) = self.context {
                //     // skip head and validate each
                //     headers.iter().skip(1).try_for_each(|h| self.validate_db_header(&h))?;
                // }
                Ok(())
            }
        }

        impl<D: HeaderDownloader + 'static> UnwindStageTestRunner for HeadersTestRunner<D> {
            fn seed_unwind(
                &mut self,
                input: UnwindInput,
                highest_entry: u64,
            ) -> Result<(), TestRunnerError> {
                let lowest_entry = input.unwind_to.saturating_sub(100);
                let headers = random_header_range(lowest_entry..highest_entry, H256::zero());
                self.insert_headers(headers.iter())?;
                Ok(())
            }

            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                let unwind_to = input.unwind_to;
                self.db.check_no_entry_above_by_value::<tables::HeaderNumbers, _>(
                    unwind_to,
                    |val| val,
                )?;
                self.db
                    .check_no_entry_above::<tables::CanonicalHeaders, _>(unwind_to, |key| key)?;
                self.db
                    .check_no_entry_above::<tables::Headers, _>(unwind_to, |key| key.number())?;
                self.db
                    .check_no_entry_above::<tables::HeaderTD, _>(unwind_to, |key| key.number())?;
                Ok(())
            }
        }

        impl HeadersTestRunner<LinearDownloader<TestConsensus, TestHeadersClient>> {
            pub(crate) fn with_linear_downloader() -> Self {
                let client = Arc::new(TestHeadersClient::default());
                let consensus = Arc::new(TestConsensus::default());
                let downloader = Arc::new(
                    LinearDownloadBuilder::default().build(consensus.clone(), client.clone()),
                );
                Self { client, consensus, downloader, db: StageTestDB::default() }
            }
        }

        impl<D: HeaderDownloader> HeadersTestRunner<D> {
            /// Insert header into tables
            pub(crate) fn insert_header(&self, header: &SealedHeader) -> Result<(), db::Error> {
                self.insert_headers(std::iter::once(header))
            }

            /// Insert headers into tables
            pub(crate) fn insert_headers<'a, I>(&self, headers: I) -> Result<(), db::Error>
            where
                I: Iterator<Item = &'a SealedHeader>,
            {
                let headers = headers.collect::<Vec<_>>();
                self.db
                    .map_put::<tables::HeaderNumbers, _, _>(&headers, |h| (h.hash(), h.number))?;
                self.db.map_put::<tables::Headers, _, _>(&headers, |h| {
                    (BlockNumHash((h.number, h.hash())), h.deref().clone().unseal())
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
                header: &SealedHeader,
            ) -> Result<(), db::Error> {
                self.db.query(|tx| {
                    let key: BlockNumHash = (header.number, header.hash()).into();

                    let db_number = tx.get::<tables::HeaderNumbers>(header.hash())?;
                    assert_eq!(db_number, Some(header.number));

                    let db_header = tx.get::<tables::Headers>(key)?;
                    assert_eq!(db_header, Some(header.clone().unseal()));

                    let db_canonical_header = tx.get::<tables::CanonicalHeaders>(header.number)?;
                    assert_eq!(db_canonical_header, Some(header.hash()));

                    if header.number != 0 {
                        let parent_key: BlockNumHash =
                            (header.number - 1, header.parent_hash).into();
                        let parent_td = tx.get::<tables::HeaderTD>(parent_key)?;
                        let td = U256::from_big_endian(&tx.get::<tables::HeaderTD>(key)?.unwrap());
                        assert_eq!(
                            parent_td.map(|td| U256::from_big_endian(&td) + header.difficulty),
                            Some(td)
                        );
                    }

                    Ok(())
                })
            }
        }
    }
}
