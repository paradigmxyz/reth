use crate::{
    db::Transaction, metrics::HeaderMetrics, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use futures_util::StreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::blocks::BlockNumHash,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    p2p::headers::{
        client::{HeadersClient, StatusUpdater},
        downloader::{HeaderDownloader, SyncTarget},
    },
};
use reth_primitives::{BlockNumber, Header, SealedHeader, U256};
use std::{fmt::Debug, sync::Arc};
use tracing::*;

pub(crate) const HEADERS: StageId = StageId("Headers");

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
///
/// NOTE: This stage downloads headers in reverse. Upon returning the control flow to the pipeline,
/// the stage progress is not updated unless this stage is done.
#[derive(Debug)]
pub struct HeaderStage<D: HeaderDownloader, C: Consensus, H: HeadersClient, S: StatusUpdater> {
    /// Strategy for downloading the headers
    pub downloader: D,
    /// Consensus client implementation
    pub consensus: Arc<C>,
    /// Downloader client implementation
    pub client: Arc<H>,
    /// Network handle for updating status
    pub network_handle: S,
    /// Header metrics
    pub metrics: HeaderMetrics,
}

// === impl HeaderStage ===

impl<D, C, H, S> HeaderStage<D, C, H, S>
where
    D: HeaderDownloader,
    C: Consensus,
    H: HeadersClient,
    S: StatusUpdater,
{
    async fn update_head<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let block_key = tx.get_block_numhash(height)?;
        let td: U256 = *tx
            .get::<tables::HeaderTD>(block_key)?
            .ok_or(DatabaseIntegrityError::TotalDifficulty { number: height })?;
        // TODO: This should happen in the last stage
        self.network_handle.update_status(height, block_key.hash(), td);
        Ok(())
    }

    fn is_stage_done<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        stage_progress: u64,
    ) -> Result<bool, StageError> {
        let mut header_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let (head_num, _) = header_cursor
            .seek_exact(stage_progress)?
            .ok_or(DatabaseIntegrityError::CanonicalHeader { number: stage_progress })?;
        // Check if the next entry is congruent
        Ok(header_cursor.next()?.map(|(next_num, _)| head_num + 1 == next_num).unwrap_or_default())
    }

    /// Get the head and tip of the range we need to sync
    ///
    /// See also [SyncTarget]
    async fn get_sync_gap<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        stage_progress: u64,
    ) -> Result<SyncGap, StageError> {
        // Create a cursor over canonical header hashes
        let mut cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let mut header_cursor = tx.cursor_read::<tables::Headers>()?;

        // Get head hash and reposition the cursor
        let (head_num, head_hash) = cursor
            .seek_exact(stage_progress)?
            .ok_or(DatabaseIntegrityError::CanonicalHeader { number: stage_progress })?;

        // Construct head
        let (_, head) = header_cursor
            .seek_exact((head_num, head_hash).into())?
            .ok_or(DatabaseIntegrityError::Header { number: head_num, hash: head_hash })?;
        let local_head = SealedHeader::new(head, head_hash);

        // Look up the next header
        let next_header = cursor
            .next()?
            .map(|(next_num, next_hash)| -> Result<Header, StageError> {
                let (_, next) = header_cursor
                    .seek_exact((next_num, next_hash).into())?
                    .ok_or(DatabaseIntegrityError::Header { number: next_num, hash: next_hash })?;
                Ok(next)
            })
            .transpose()?;

        // Decide the tip or error out on invalid input.
        // If the next element found in the cursor is not the "expected" next block per our current
        // progress, then there is a gap in the database and we should start downloading in
        // reverse from there. Else, it should use whatever the forkchoice state reports.
        let target = match next_header {
            Some(header) if stage_progress + 1 != header.number => SyncTarget::Gap(header.seal()),
            None => SyncTarget::Tip(self.next_fork_choice_state().await.head_block_hash),
            _ => return Err(StageError::StageProgress(stage_progress)),
        };

        Ok(SyncGap { local_head, target })
    }

    /// Awaits the next [ForkchoiceState] message from [Consensus] with a non-zero block hash
    async fn next_fork_choice_state(&self) -> ForkchoiceState {
        let mut state_rcv = self.consensus.fork_choice_state();
        loop {
            let _ = state_rcv.changed().await;
            let forkchoice = state_rcv.borrow();
            if !forkchoice.head_block_hash.is_zero() {
                return forkchoice.clone()
            }
        }
    }

    /// Write downloaded headers to the given transaction
    ///
    /// Note: this writes the headers with rising block numbers.
    fn write_headers<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        headers: Vec<SealedHeader>,
    ) -> Result<Option<BlockNumber>, StageError> {
        trace!(target: "sync::stages::headers", len = headers.len(), "writing headers");

        let mut cursor_header = tx.cursor_write::<tables::Headers>()?;
        let mut cursor_canonical = tx.cursor_write::<tables::CanonicalHeaders>()?;

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

            // NOTE: HeaderNumbers are not sorted and can't be inserted with cursor.
            tx.put::<tables::HeaderNumbers>(block_hash, header.number)?;
            cursor_header.insert(key, header)?;
            cursor_canonical.insert(key.number(), key.hash())?;
        }
        Ok(latest)
    }
}

#[async_trait::async_trait]
impl<DB, D, C, H, S> Stage<DB> for HeaderStage<D, C, H, S>
where
    DB: Database,
    D: HeaderDownloader,
    C: Consensus,
    H: HeadersClient,
    S: StatusUpdater,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        HEADERS
    }

    /// Download the headers in reverse order (falling block numbers)
    /// starting from the tip of the chain
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let current_progress = input.stage_progress.unwrap_or_default();
        self.update_head::<DB>(tx, current_progress).await?;

        // Lookup the head and tip of the sync range
        let gap = self.get_sync_gap(tx, current_progress).await?;
        let tip = gap.target.tip();

        // Nothing to sync
        if gap.is_closed() {
            info!(target: "sync::stages::headers", stage_progress = current_progress, target = ?tip, "Target block already reached");
            return Ok(ExecOutput { stage_progress: current_progress, done: true })
        }

        debug!(target: "sync::stages::headers", ?tip, head = ?gap.local_head.hash(), "Commencing sync");

        // let the downloader know what to sync
        self.downloader.update_sync_gap(gap.local_head, gap.target);

        // The downloader returns the headers in descending order starting from the tip
        // down to the local head (latest block in db)
        let downloaded_headers = match self.downloader.next().await {
            Some(downloaded_headers) => downloaded_headers,
            None => {
                info!(target: "sync::stages::headers", stage_progress = current_progress, target = ?tip, "Download stream exhausted");
                return Ok(ExecOutput { stage_progress: current_progress, done: true })
            }
        };

        info!(target: "sync::stages::headers", len = downloaded_headers.len(), "Received headers");
        self.metrics.headers_counter.increment(downloaded_headers.len() as u64);

        // Write the headers to db
        self.write_headers::<DB>(tx, downloaded_headers)?.unwrap_or_default();

        if self.is_stage_done(tx, current_progress)? {
            let stage_progress = current_progress.max(
                tx.cursor_read::<tables::CanonicalHeaders>()?
                    .last()?
                    .map(|(num, _)| num)
                    .unwrap_or_default(),
            );
            Ok(ExecOutput { stage_progress, done: true })
        } else {
            Ok(ExecOutput { stage_progress: current_progress, done: false })
        }
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // TODO: handle bad block
        info!(target: "sync::stages::headers", to_block = input.unwind_to, "Unwinding");
        tx.unwind_table_by_walker::<tables::CanonicalHeaders, tables::HeaderNumbers>(
            input.unwind_to + 1,
        )?;
        tx.unwind_table_by_num::<tables::CanonicalHeaders>(input.unwind_to)?;
        tx.unwind_table_by_num_hash::<tables::Headers>(input.unwind_to)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

/// Represents a gap to sync: from `local_head` to `target`
#[derive(Debug)]
struct SyncGap {
    local_head: SealedHeader,
    target: SyncTarget,
}

// === impl SyncGap ===

impl SyncGap {
    /// Returns `true` if the gap from the head to the target was closed
    #[inline]
    fn is_closed(&self) -> bool {
        self.local_head.hash() == self.target.tip()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestRunner, UnwindStageTestRunner,
        PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::random_header;
    use reth_primitives::H256;
    use test_runner::HeadersTestRunner;

    mod test_runner {
        use crate::{
            metrics::HeaderMetrics,
            stages::headers::HeaderStage,
            test_utils::{
                ExecuteStageTestRunner, StageTestRunner, TestRunnerError, TestTransaction,
                UnwindStageTestRunner,
            },
            ExecInput, ExecOutput, UnwindInput,
        };
        use reth_db::{
            models::blocks::BlockNumHash,
            tables,
            transaction::{DbTx, DbTxMut},
        };
        use reth_downloaders::headers::linear::{LinearDownloadBuilder, LinearDownloader};
        use reth_interfaces::{
            p2p::headers::downloader::HeaderDownloader,
            test_utils::{
                generators::{random_header, random_header_range},
                TestConsensus, TestHeaderDownloader, TestHeadersClient, TestStatusUpdater,
            },
        };
        use reth_primitives::{BlockNumber, SealedHeader, U256};
        use std::sync::Arc;

        pub(crate) struct HeadersTestRunner<D: HeaderDownloader> {
            pub(crate) consensus: Arc<TestConsensus>,
            pub(crate) client: Arc<TestHeadersClient>,
            downloader_factory: Box<dyn Fn() -> D + Send + Sync + 'static>,
            network_handle: TestStatusUpdater,
            tx: TestTransaction,
        }

        impl Default for HeadersTestRunner<TestHeaderDownloader> {
            fn default() -> Self {
                let client = Arc::new(TestHeadersClient::default());
                let consensus = Arc::new(TestConsensus::default());
                Self {
                    client: client.clone(),
                    consensus: consensus.clone(),
                    downloader_factory: Box::new(move || {
                        TestHeaderDownloader::new(client.clone(), consensus.clone(), 1000, 1000)
                    }),
                    network_handle: TestStatusUpdater::default(),
                    tx: TestTransaction::default(),
                }
            }
        }

        impl<D: HeaderDownloader + 'static> StageTestRunner for HeadersTestRunner<D> {
            type S = HeaderStage<D, TestConsensus, TestHeadersClient, TestStatusUpdater>;

            fn tx(&self) -> &TestTransaction {
                &self.tx
            }

            fn stage(&self) -> Self::S {
                HeaderStage {
                    consensus: self.consensus.clone(),
                    client: self.client.clone(),
                    downloader: (*self.downloader_factory)(),
                    network_handle: self.network_handle.clone(),
                    metrics: HeaderMetrics::default(),
                }
            }
        }

        #[async_trait::async_trait]
        impl<D: HeaderDownloader + 'static> ExecuteStageTestRunner for HeadersTestRunner<D> {
            type Seed = Vec<SealedHeader>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let start = input.stage_progress.unwrap_or_default();
                let head = random_header(start, None);
                self.tx.insert_headers(std::iter::once(&head))?;
                // patch td table for `update_head` call
                self.tx.commit(|tx| {
                    tx.put::<tables::HeaderTD>(head.num_hash().into(), U256::ZERO.into())
                })?;

                // use previous progress as seed size
                let end = input.previous_stage.map(|(_, num)| num).unwrap_or_default() + 1;

                if start + 1 >= end {
                    return Ok(Vec::default())
                }

                let mut headers = random_header_range(start + 1..end, head.hash());
                headers.insert(0, head);
                Ok(headers)
            }

            /// Validate stored headers
            fn validate_execution(
                &self,
                input: ExecInput,
                output: Option<ExecOutput>,
            ) -> Result<(), TestRunnerError> {
                let initial_stage_progress = input.stage_progress.unwrap_or_default();
                match output {
                    Some(output) if output.stage_progress > initial_stage_progress => {
                        self.tx.query(|tx| {
                            for block_num in (initial_stage_progress..output.stage_progress).rev() {
                                // look up the header hash
                                let hash = tx
                                    .get::<tables::CanonicalHeaders>(block_num)?
                                    .expect("no header hash");
                                let key: BlockNumHash = (block_num, hash).into();

                                // validate the header number
                                assert_eq!(tx.get::<tables::HeaderNumbers>(hash)?, Some(block_num));

                                // validate the header
                                let header = tx.get::<tables::Headers>(key)?;
                                assert!(header.is_some());
                                let header = header.unwrap().seal();
                                assert_eq!(header.hash(), hash);
                            }
                            Ok(())
                        })?;
                    }
                    _ => self.check_no_header_entry_above(initial_stage_progress)?,
                };
                Ok(())
            }

            async fn after_execution(&self, headers: Self::Seed) -> Result<(), TestRunnerError> {
                self.client.extend(headers.iter().map(|h| h.clone().unseal())).await;
                let tip = if !headers.is_empty() {
                    headers.last().unwrap().hash()
                } else {
                    let tip = random_header(0, None);
                    self.tx.insert_headers(std::iter::once(&tip))?;
                    tip.hash()
                };
                self.consensus.update_tip(tip);
                Ok(())
            }
        }

        impl<D: HeaderDownloader + 'static> UnwindStageTestRunner for HeadersTestRunner<D> {
            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                self.check_no_header_entry_above(input.unwind_to)
            }
        }

        impl HeadersTestRunner<LinearDownloader<TestConsensus, TestHeadersClient>> {
            pub(crate) fn with_linear_downloader() -> Self {
                let client = Arc::new(TestHeadersClient::default());
                let consensus = Arc::new(TestConsensus::default());
                Self {
                    client: client.clone(),
                    consensus: consensus.clone(),
                    downloader_factory: Box::new(move || {
                        LinearDownloadBuilder::default().stream_batch_size(500).build(
                            consensus.clone(),
                            client.clone(),
                            Default::default(),
                            Default::default(),
                        )
                    }),
                    network_handle: TestStatusUpdater::default(),
                    tx: TestTransaction::default(),
                }
            }
        }

        impl<D: HeaderDownloader> HeadersTestRunner<D> {
            pub(crate) fn check_no_header_entry_above(
                &self,
                block: BlockNumber,
            ) -> Result<(), TestRunnerError> {
                self.tx
                    .check_no_entry_above_by_value::<tables::HeaderNumbers, _>(block, |val| val)?;
                self.tx.check_no_entry_above::<tables::CanonicalHeaders, _>(block, |key| key)?;
                self.tx.check_no_entry_above::<tables::Headers, _>(block, |key| key.number())?;
                Ok(())
            }
        }
    }

    stage_test_suite!(HeadersTestRunner, headers);

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

        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        let tip = headers.last().unwrap();
        runner.consensus.update_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { done: true, stage_progress }) if stage_progress == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }

    /// Test the head and tip range lookup
    #[tokio::test]
    async fn head_and_tip_lookup() {
        let runner = HeadersTestRunner::default();
        let tx = runner.tx().inner();
        let stage = runner.stage();

        let consensus_tip = H256::random();
        stage.consensus.update_tip(consensus_tip);

        // Genesis
        let stage_progress = 0;
        let head = random_header(0, None);
        let gap_fill = random_header(1, Some(head.hash()));
        let gap_tip = random_header(2, Some(gap_fill.hash()));

        // Empty database
        assert_matches!(
            stage.get_sync_gap(&tx, stage_progress).await,
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::CanonicalHeader { number }))
                if number == stage_progress
        );

        // Checkpoint and no gap
        tx.put::<tables::CanonicalHeaders>(head.number, head.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(head.num_hash().into(), head.clone().unseal())
            .expect("failed to write header");

        let gap = stage.get_sync_gap(&tx, stage_progress).await.unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), consensus_tip);

        // Checkpoint and gap
        tx.put::<tables::CanonicalHeaders>(gap_tip.number, gap_tip.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(gap_tip.num_hash().into(), gap_tip.clone().unseal())
            .expect("failed to write header");

        let gap = stage.get_sync_gap(&tx, stage_progress).await.unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), gap_tip.parent_hash);

        // Checkpoint and gap closed
        tx.put::<tables::CanonicalHeaders>(gap_fill.number, gap_fill.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(gap_fill.num_hash().into(), gap_fill.clone().unseal())
            .expect("failed to write header");

        assert_matches!(
            stage.get_sync_gap(&tx, stage_progress).await,
            Err(StageError::StageProgress(progress)) if progress == stage_progress
        );
    }

    /// Execute the stage in two steps
    #[tokio::test]
    async fn execute_from_previous_progress() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        // pick range that's larger than the configured headers batch size
        let (stage_progress, previous_stage) = (600, 1200);
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        let tip = headers.last().unwrap();
        runner.consensus.update_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { done: false, stage_progress: progress }) if progress == stage_progress);

        runner.client.clear().await;
        runner.client.extend(headers.iter().take(101).map(|h| h.clone().unseal()).rev()).await;

        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(result, Ok(ExecOutput { done: true, stage_progress }) if stage_progress == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }
}
