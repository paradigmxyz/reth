use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use futures_util::StreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{
    p2p::headers::{downloader::HeaderDownloader, error::HeadersDownloaderError},
    provider::ProviderError,
};
use reth_primitives::{
    stage::{
        CheckpointBlockRange, EntitiesCheckpoint, HeadersCheckpoint, StageCheckpoint, StageId,
    },
    BlockHashOrNumber, BlockNumber, SealedHeader,
};
use reth_provider::{DatabaseProviderRW, HeaderSyncGap, HeaderSyncGapProvider, HeaderSyncMode};
use std::task::{ready, Context, Poll};
use tracing::*;

/// The headers stage.
///
/// The headers stage downloads all block headers from the highest block in the local database to
/// the perceived highest block on the network.
///
/// The headers are processed and data is inserted into these tables:
///
/// - [`HeaderNumbers`][reth_db::tables::HeaderNumbers]
/// - [`Headers`][reth_db::tables::Headers]
/// - [`CanonicalHeaders`][reth_db::tables::CanonicalHeaders]
///
/// NOTE: This stage downloads headers in reverse. Upon returning the control flow to the pipeline,
/// the stage checkpoint is not updated until this stage is done.
#[derive(Debug)]
pub struct HeaderStage<Provider, Downloader: HeaderDownloader> {
    /// Database handle.
    provider: Provider,
    /// Strategy for downloading the headers
    downloader: Downloader,
    /// The sync mode for the stage.
    mode: HeaderSyncMode,
    /// Current sync gap.
    sync_gap: Option<HeaderSyncGap>,
    /// Header buffer.
    buffer: Option<Vec<SealedHeader>>,
}

// === impl HeaderStage ===

impl<Provider, Downloader> HeaderStage<Provider, Downloader>
where
    Downloader: HeaderDownloader,
{
    /// Create a new header stage
    pub fn new(database: Provider, downloader: Downloader, mode: HeaderSyncMode) -> Self {
        Self { provider: database, downloader, mode, sync_gap: None, buffer: None }
    }

    fn is_stage_done<DB: Database>(
        &self,
        tx: &<DB as Database>::TXMut,
        checkpoint: u64,
    ) -> Result<bool, StageError> {
        let mut header_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let (head_num, _) = header_cursor
            .seek_exact(checkpoint)?
            .ok_or_else(|| ProviderError::HeaderNotFound(checkpoint.into()))?;
        // Check if the next entry is congruent
        Ok(header_cursor.next()?.map(|(next_num, _)| head_num + 1 == next_num).unwrap_or_default())
    }

    /// Write downloaded headers to the given transaction
    ///
    /// Note: this writes the headers with rising block numbers.
    fn write_headers<DB: Database>(
        &self,
        tx: &<DB as Database>::TXMut,
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

            let header_hash = header.hash();
            let header_number = header.number;
            let header = header.unseal();
            latest = Some(header.number);

            // NOTE: HeaderNumbers are not sorted and can't be inserted with cursor.
            tx.put::<tables::HeaderNumbers>(header_hash, header_number)?;
            cursor_header.insert(header_number, header)?;
            cursor_canonical.insert(header_number, header_hash)?;
        }

        Ok(latest)
    }
}

impl<DB, Provider, D> Stage<DB> for HeaderStage<Provider, D>
where
    DB: Database,
    Provider: HeaderSyncGapProvider,
    D: HeaderDownloader,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::Headers
    }

    fn poll_execute_ready(
        &mut self,
        cx: &mut Context<'_>,
        input: ExecInput,
    ) -> Poll<Result<(), StageError>> {
        let current_checkpoint = input.checkpoint();

        // Return if buffer already has some items.
        if self.buffer.is_some() {
            // TODO: review
            trace!(
                target: "sync::stages::headers",
                checkpoint = %current_checkpoint.block_number,
                "Buffer is not empty"
            );
            return Poll::Ready(Ok(()))
        }

        // Lookup the head and tip of the sync range
        let gap = self.provider.sync_gap(self.mode.clone(), current_checkpoint.block_number)?;
        let tip = gap.target.tip();
        self.sync_gap = Some(gap.clone());

        // Nothing to sync
        if gap.is_closed() {
            info!(
                target: "sync::stages::headers",
                checkpoint = %current_checkpoint.block_number,
                target = ?tip,
                "Target block already reached"
            );
            return Poll::Ready(Ok(()))
        }

        debug!(target: "sync::stages::headers", ?tip, head = ?gap.local_head.hash(), "Commencing sync");

        // let the downloader know what to sync
        self.downloader.update_sync_gap(gap.local_head, gap.target);

        let result = match ready!(self.downloader.poll_next_unpin(cx)) {
            Some(Ok(headers)) => {
                info!(target: "sync::stages::headers", len = headers.len(), "Received headers");
                self.buffer = Some(headers);
                Ok(())
            }
            Some(Err(HeadersDownloaderError::DetachedHead { local_head, header, error })) => {
                error!(target: "sync::stages::headers", ?error, "Cannot attach header to head");
                Err(StageError::DetachedHead { local_head, header, error })
            }
            None => Err(StageError::ChannelClosed),
        };
        Poll::Ready(result)
    }

    /// Download the headers in reverse order (falling block numbers)
    /// starting from the tip of the chain
    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let current_checkpoint = input.checkpoint();

        let gap = self.sync_gap.clone().ok_or(StageError::MissingSyncGap)?;
        if gap.is_closed() {
            return Ok(ExecOutput::done(current_checkpoint))
        }

        let local_head = gap.local_head.number;
        let tip = gap.target.tip();

        let downloaded_headers = self.buffer.take().ok_or(StageError::MissingDownloadBuffer)?;
        let tip_block_number = match tip {
            // If tip is hash and it equals to the first downloaded header's hash, we can use
            // the block number of this header as tip.
            BlockHashOrNumber::Hash(hash) => downloaded_headers
                .first()
                .and_then(|header| (header.hash == hash).then_some(header.number)),
            // If tip is number, we can just grab it and not resolve using downloaded headers.
            BlockHashOrNumber::Number(number) => Some(number),
        };

        // Since we're syncing headers in batches, gap tip will move in reverse direction towards
        // our local head with every iteration. To get the actual target block number we're
        // syncing towards, we need to take into account already synced headers from the database.
        // It is `None`, if tip didn't change and we're still downloading headers for previously
        // calculated gap.
        let tx = provider.tx_ref();
        let target_block_number = if let Some(tip_block_number) = tip_block_number {
            let local_max_block_number = tx
                .cursor_read::<tables::CanonicalHeaders>()?
                .last()?
                .map(|(canonical_block, _)| canonical_block);

            Some(tip_block_number.max(local_max_block_number.unwrap_or_default()))
        } else {
            None
        };

        let mut stage_checkpoint = match current_checkpoint.headers_stage_checkpoint() {
            // If checkpoint block range matches our range, we take the previously used
            // stage checkpoint as-is.
            Some(stage_checkpoint)
                if stage_checkpoint.block_range.from == input.checkpoint().block_number =>
            {
                stage_checkpoint
            }
            // Otherwise, we're on the first iteration of new gap sync, so we recalculate the number
            // of already processed and total headers.
            // `target_block_number` is guaranteed to be `Some`, because on the first iteration
            // we download the header for missing tip and use its block number.
            _ => {
                let target = target_block_number.expect("No downloaded header for tip found");
                HeadersCheckpoint {
                    block_range: CheckpointBlockRange {
                        from: input.checkpoint().block_number,
                        to: target,
                    },
                    progress: EntitiesCheckpoint {
                        // Set processed to the local head block number + number
                        // of block already filled in the gap.
                        processed: local_head + (target - tip_block_number.unwrap_or_default()),
                        total: target,
                    },
                }
            }
        };

        // Total headers can be updated if we received new tip from the network, and need to fill
        // the local gap.
        if let Some(target_block_number) = target_block_number {
            stage_checkpoint.progress.total = target_block_number;
        }
        stage_checkpoint.progress.processed += downloaded_headers.len() as u64;

        // Write the headers to db
        self.write_headers::<DB>(tx, downloaded_headers)?.unwrap_or_default();

        if self.is_stage_done::<DB>(tx, current_checkpoint.block_number)? {
            let checkpoint = current_checkpoint.block_number.max(
                tx.cursor_read::<tables::CanonicalHeaders>()?
                    .last()?
                    .map(|(num, _)| num)
                    .unwrap_or_default(),
            );
            Ok(ExecOutput {
                checkpoint: StageCheckpoint::new(checkpoint)
                    .with_headers_stage_checkpoint(stage_checkpoint),
                done: true,
            })
        } else {
            Ok(ExecOutput {
                checkpoint: current_checkpoint.with_headers_stage_checkpoint(stage_checkpoint),
                done: false,
            })
        }
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.buffer.take();
        self.sync_gap.take();

        provider.unwind_table_by_walker::<tables::CanonicalHeaders, tables::HeaderNumbers>(
            input.unwind_to + 1,
        )?;
        provider.unwind_table_by_num::<tables::CanonicalHeaders>(input.unwind_to)?;
        let unwound_headers = provider.unwind_table_by_num::<tables::Headers>(input.unwind_to)?;

        let stage_checkpoint =
            input.checkpoint.headers_stage_checkpoint().map(|stage_checkpoint| HeadersCheckpoint {
                block_range: stage_checkpoint.block_range,
                progress: EntitiesCheckpoint {
                    processed: stage_checkpoint
                        .progress
                        .processed
                        .saturating_sub(unwound_headers as u64),
                    total: stage_checkpoint.progress.total,
                },
            });

        let mut checkpoint = StageCheckpoint::new(input.unwind_to);
        if let Some(stage_checkpoint) = stage_checkpoint {
            checkpoint = checkpoint.with_headers_stage_checkpoint(stage_checkpoint);
        }

        Ok(UnwindOutput { checkpoint })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestRunner, UnwindStageTestRunner,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::random_header;
    use reth_primitives::{stage::StageUnitCheckpoint, B256};
    use reth_provider::ProviderFactory;
    use test_runner::HeadersTestRunner;

    mod test_runner {
        use super::*;
        use crate::test_utils::{TestRunnerError, TestStageDB};
        use reth_db::{test_utils::TempDatabase, DatabaseEnv};
        use reth_downloaders::headers::reverse_headers::{
            ReverseHeadersDownloader, ReverseHeadersDownloaderBuilder,
        };
        use reth_interfaces::test_utils::{
            generators, generators::random_header_range, TestConsensus, TestHeaderDownloader,
            TestHeadersClient,
        };
        use reth_primitives::U256;
        use reth_provider::{BlockHashReader, BlockNumReader, HeaderProvider};
        use std::sync::Arc;
        use tokio::sync::watch;

        pub(crate) struct HeadersTestRunner<D: HeaderDownloader> {
            pub(crate) client: TestHeadersClient,
            channel: (watch::Sender<B256>, watch::Receiver<B256>),
            downloader_factory: Box<dyn Fn() -> D + Send + Sync + 'static>,
            db: TestStageDB,
        }

        impl Default for HeadersTestRunner<TestHeaderDownloader> {
            fn default() -> Self {
                let client = TestHeadersClient::default();
                Self {
                    client: client.clone(),
                    channel: watch::channel(B256::ZERO),
                    downloader_factory: Box::new(move || {
                        TestHeaderDownloader::new(
                            client.clone(),
                            Arc::new(TestConsensus::default()),
                            1000,
                            1000,
                        )
                    }),
                    db: TestStageDB::default(),
                }
            }
        }

        impl<D: HeaderDownloader + 'static> StageTestRunner for HeadersTestRunner<D> {
            type S = HeaderStage<ProviderFactory<Arc<TempDatabase<DatabaseEnv>>>, D>;

            fn db(&self) -> &TestStageDB {
                &self.db
            }

            fn stage(&self) -> Self::S {
                HeaderStage::new(
                    self.db.factory.clone(),
                    (*self.downloader_factory)(),
                    HeaderSyncMode::Tip(self.channel.1.clone()),
                )
            }
        }

        #[async_trait::async_trait]
        impl<D: HeaderDownloader + 'static> ExecuteStageTestRunner for HeadersTestRunner<D> {
            type Seed = Vec<SealedHeader>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let mut rng = generators::rng();
                let start = input.checkpoint().block_number;
                let head = random_header(&mut rng, start, None);
                self.db.insert_headers(std::iter::once(&head))?;
                // patch td table for `update_head` call
                self.db
                    .commit(|tx| Ok(tx.put::<tables::HeaderTD>(head.number, U256::ZERO.into())?))?;

                // use previous checkpoint as seed size
                let end = input.target.unwrap_or_default() + 1;

                if start + 1 >= end {
                    return Ok(Vec::default())
                }

                let mut headers = random_header_range(&mut rng, start + 1..end, head.hash());
                headers.insert(0, head);
                Ok(headers)
            }

            /// Validate stored headers
            fn validate_execution(
                &self,
                input: ExecInput,
                output: Option<ExecOutput>,
            ) -> Result<(), TestRunnerError> {
                let initial_checkpoint = input.checkpoint().block_number;
                match output {
                    Some(output) if output.checkpoint.block_number > initial_checkpoint => {
                        let provider = self.db.factory.provider()?;
                        for block_num in (initial_checkpoint..output.checkpoint.block_number).rev()
                        {
                            // look up the header hash
                            let hash = provider.block_hash(block_num)?.expect("no header hash");

                            // validate the header number
                            assert_eq!(provider.block_number(hash)?, Some(block_num));

                            // validate the header
                            let header = provider.header_by_number(block_num)?;
                            assert!(header.is_some());
                            let header = header.unwrap().seal_slow();
                            assert_eq!(header.hash(), hash);
                        }
                    }
                    _ => self.check_no_header_entry_above(initial_checkpoint)?,
                };
                Ok(())
            }

            async fn after_execution(&self, headers: Self::Seed) -> Result<(), TestRunnerError> {
                self.client.extend(headers.iter().map(|h| h.clone().unseal())).await;
                let tip = if !headers.is_empty() {
                    headers.last().unwrap().hash()
                } else {
                    let tip = random_header(&mut generators::rng(), 0, None);
                    self.db.insert_headers(std::iter::once(&tip))?;
                    tip.hash()
                };
                self.send_tip(tip);
                Ok(())
            }
        }

        impl<D: HeaderDownloader + 'static> UnwindStageTestRunner for HeadersTestRunner<D> {
            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                self.check_no_header_entry_above(input.unwind_to)
            }
        }

        impl HeadersTestRunner<ReverseHeadersDownloader<TestHeadersClient>> {
            pub(crate) fn with_linear_downloader() -> Self {
                let client = TestHeadersClient::default();
                Self {
                    client: client.clone(),
                    channel: watch::channel(B256::ZERO),
                    downloader_factory: Box::new(move || {
                        ReverseHeadersDownloaderBuilder::default()
                            .stream_batch_size(500)
                            .build(client.clone(), Arc::new(TestConsensus::default()))
                    }),
                    db: TestStageDB::default(),
                }
            }
        }

        impl<D: HeaderDownloader> HeadersTestRunner<D> {
            pub(crate) fn check_no_header_entry_above(
                &self,
                block: BlockNumber,
            ) -> Result<(), TestRunnerError> {
                self.db
                    .ensure_no_entry_above_by_value::<tables::HeaderNumbers, _>(block, |val| val)?;
                self.db.ensure_no_entry_above::<tables::CanonicalHeaders, _>(block, |key| key)?;
                self.db.ensure_no_entry_above::<tables::Headers, _>(block, |key| key)?;
                Ok(())
            }

            pub(crate) fn send_tip(&self, tip: B256) {
                self.channel.0.send(tip).expect("failed to send tip");
            }
        }
    }

    stage_test_suite!(HeadersTestRunner, headers);

    /// Execute the stage with linear downloader
    #[tokio::test]
    async fn execute_with_linear_downloader() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        let (checkpoint, previous_stage) = (1000, 1200);
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(checkpoint)),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        let tip = headers.last().unwrap();
        runner.send_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!( result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Headers(HeadersCheckpoint {
                block_range: CheckpointBlockRange {
                    from,
                    to
                },
                progress: EntitiesCheckpoint {
                    processed,
                    total,
                }
            }))
        }, done: true }) if block_number == tip.number &&
            from == checkpoint && to == previous_stage &&
            // -1 because we don't need to download the local head
            processed == checkpoint + headers.len() as u64 - 1 && total == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }

    /// Execute the stage in two steps
    #[tokio::test]
    async fn execute_from_previous_checkpoint() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        // pick range that's larger than the configured headers batch size
        let (checkpoint, previous_stage) = (600, 1200);
        let mut input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(checkpoint)),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        let tip = headers.last().unwrap();
        runner.send_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Headers(HeadersCheckpoint {
                block_range: CheckpointBlockRange {
                    from,
                    to
                },
                progress: EntitiesCheckpoint {
                    processed,
                    total,
                }
            }))
        }, done: false }) if block_number == checkpoint &&
            from == checkpoint && to == previous_stage &&
            processed == checkpoint + 500 && total == tip.number);

        runner.client.clear().await;
        runner.client.extend(headers.iter().take(101).map(|h| h.clone().unseal()).rev()).await;
        input.checkpoint = Some(result.unwrap().checkpoint);

        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Headers(HeadersCheckpoint {
                block_range: CheckpointBlockRange {
                    from,
                    to
                },
                progress: EntitiesCheckpoint {
                    processed,
                    total,
                }
            }))
        }, done: true }) if block_number == tip.number &&
            from == checkpoint && to == previous_stage &&
            // -1 because we don't need to download the local head
            processed == checkpoint + headers.len() as u64 - 1 && total == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }
}
