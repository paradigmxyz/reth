use futures_util::StreamExt;
use reth_codecs::Compact;
use reth_config::config::EtlConfig;
use reth_consensus::Consensus;
use reth_db::{tables, RawKey, RawTable, RawValue};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    transaction::DbTxMut,
};
use reth_etl::Collector;
use reth_network_p2p::headers::{downloader::HeaderDownloader, error::HeadersDownloaderError};
use reth_primitives::{BlockHash, BlockNumber, SealedHeader, StaticFileSegment, B256};
use reth_provider::{
    providers::{StaticFileProvider, StaticFileWriter},
    BlockHashReader, DatabaseProviderRW, HeaderProvider, HeaderSyncGap, HeaderSyncGapProvider,
};
use reth_stages_api::{
    BlockErrorKind, CheckpointBlockRange, EntitiesCheckpoint, ExecInput, ExecOutput,
    HeadersCheckpoint, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_storage_errors::provider::ProviderError;
use std::{
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::watch;
use tracing::*;

/// The headers stage.
///
/// The headers stage downloads all block headers from the highest block in storage to
/// the perceived highest block on the network.
///
/// The headers are processed and data is inserted into static files, as well as into the
/// [`HeaderNumbers`][reth_db::tables::HeaderNumbers] table.
///
/// NOTE: This stage downloads headers in reverse and pushes them to the ETL [`Collector`]. It then
/// proceeds to push them sequentially to static files. The stage checkpoint is not updated until
/// this stage is done.
#[derive(Debug)]
pub struct HeaderStage<Provider, Downloader: HeaderDownloader> {
    /// Database handle.
    provider: Provider,
    /// Strategy for downloading the headers
    downloader: Downloader,
    /// The tip for the stage.
    tip: watch::Receiver<B256>,
    /// Consensus client implementation
    consensus: Arc<dyn Consensus>,
    /// Current sync gap.
    sync_gap: Option<HeaderSyncGap>,
    /// ETL collector with `HeaderHash` -> `BlockNumber`
    hash_collector: Collector<BlockHash, BlockNumber>,
    /// ETL collector with `BlockNumber` -> `SealedHeader`
    header_collector: Collector<BlockNumber, SealedHeader>,
    /// Returns true if the ETL collector has all necessary headers to fill the gap.
    is_etl_ready: bool,
}

// === impl HeaderStage ===

impl<Provider, Downloader> HeaderStage<Provider, Downloader>
where
    Downloader: HeaderDownloader,
{
    /// Create a new header stage
    pub fn new(
        database: Provider,
        downloader: Downloader,
        tip: watch::Receiver<B256>,
        consensus: Arc<dyn Consensus>,
        etl_config: EtlConfig,
    ) -> Self {
        Self {
            provider: database,
            downloader,
            tip,
            consensus,
            sync_gap: None,
            hash_collector: Collector::new(etl_config.file_size / 2, etl_config.dir.clone()),
            header_collector: Collector::new(etl_config.file_size / 2, etl_config.dir),
            is_etl_ready: false,
        }
    }

    /// Write downloaded headers to storage from ETL.
    ///
    /// Writes to static files ( `Header | HeaderTD | HeaderHash` ) and [`tables::HeaderNumbers`]
    /// database table.
    fn write_headers<DB: Database>(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        static_file_provider: StaticFileProvider,
    ) -> Result<BlockNumber, StageError> {
        let total_headers = self.header_collector.len();

        info!(target: "sync::stages::headers", total = total_headers, "Writing headers");

        // Consistency check of expected headers in static files vs DB is done on provider::sync_gap
        // when poll_execute_ready is polled.
        let mut last_header_number = static_file_provider
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .unwrap_or_default();

        // Find the latest total difficulty
        let mut td = static_file_provider
            .header_td_by_number(last_header_number)?
            .ok_or(ProviderError::TotalDifficultyNotFound(last_header_number))?;

        // Although headers were downloaded in reverse order, the collector iterates it in ascending
        // order
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;
        let interval = (total_headers / 10).max(1);
        for (index, header) in self.header_collector.iter()?.enumerate() {
            let (_, header_buf) = header?;

            if index > 0 && index % interval == 0 && total_headers > 100 {
                info!(target: "sync::stages::headers", progress = %format!("{:.2}%", (index as f64 / total_headers as f64) * 100.0), "Writing headers");
            }

            let (sealed_header, _) = SealedHeader::from_compact(&header_buf, header_buf.len());
            let (header, header_hash) = sealed_header.split();
            if header.number == 0 {
                continue
            }
            last_header_number = header.number;

            // Increase total difficulty
            td += header.difficulty;

            // Header validation
            self.consensus.validate_header_with_total_difficulty(&header, td).map_err(|error| {
                StageError::Block {
                    block: Box::new(header.clone().seal(header_hash)),
                    error: BlockErrorKind::Validation(error),
                }
            })?;

            // Append to Headers segment
            writer.append_header(&header, td, &header_hash)?;
        }

        info!(target: "sync::stages::headers", total = total_headers, "Writing headers hash index");

        let mut cursor_header_numbers =
            provider.tx_ref().cursor_write::<RawTable<tables::HeaderNumbers>>()?;
        let mut first_sync = false;

        // If we only have the genesis block hash, then we are at first sync, and we can remove it,
        // add it to the collector and use tx.append on all hashes.
        if let Some((hash, block_number)) = cursor_header_numbers.last()? {
            if block_number.value()? == 0 {
                self.hash_collector.insert(hash.key()?, 0)?;
                cursor_header_numbers.delete_current()?;
                first_sync = true;
            }
        }

        // Since ETL sorts all entries by hashes, we are either appending (first sync) or inserting
        // in order (further syncs).
        for (index, hash_to_number) in self.hash_collector.iter()?.enumerate() {
            let (hash, number) = hash_to_number?;

            if index > 0 && index % interval == 0 && total_headers > 100 {
                info!(target: "sync::stages::headers", progress = %format!("{:.2}%", (index as f64 / total_headers as f64) * 100.0), "Writing headers hash index");
            }

            if first_sync {
                cursor_header_numbers.append(
                    RawKey::<BlockHash>::from_vec(hash),
                    RawValue::<BlockNumber>::from_vec(number),
                )?;
            } else {
                cursor_header_numbers.insert(
                    RawKey::<BlockHash>::from_vec(hash),
                    RawValue::<BlockNumber>::from_vec(number),
                )?;
            }
        }

        Ok(last_header_number)
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

        // Return if stage has already completed the gap on the ETL files
        if self.is_etl_ready {
            return Poll::Ready(Ok(()))
        }

        // Lookup the head and tip of the sync range
        let gap = self.provider.sync_gap(self.tip.clone(), current_checkpoint.block_number)?;
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
            self.is_etl_ready = true;
            return Poll::Ready(Ok(()))
        }

        debug!(target: "sync::stages::headers", ?tip, head = ?gap.local_head.hash(), "Commencing sync");
        let local_head_number = gap.local_head.number;

        // let the downloader know what to sync
        self.downloader.update_sync_gap(gap.local_head, gap.target);

        // We only want to stop once we have all the headers on ETL filespace (disk).
        loop {
            match ready!(self.downloader.poll_next_unpin(cx)) {
                Some(Ok(headers)) => {
                    info!(target: "sync::stages::headers", total = headers.len(), from_block = headers.first().map(|h| h.number), to_block = headers.last().map(|h| h.number), "Received headers");
                    for header in headers {
                        let header_number = header.number;

                        self.hash_collector.insert(header.hash(), header_number)?;
                        self.header_collector.insert(header_number, header)?;

                        // Headers are downloaded in reverse, so if we reach here, we know we have
                        // filled the gap.
                        if header_number == local_head_number + 1 {
                            self.is_etl_ready = true;
                            return Poll::Ready(Ok(()))
                        }
                    }
                }
                Some(Err(HeadersDownloaderError::DetachedHead { local_head, header, error })) => {
                    error!(target: "sync::stages::headers", %error, "Cannot attach header to head");
                    return Poll::Ready(Err(StageError::DetachedHead { local_head, header, error }))
                }
                None => return Poll::Ready(Err(StageError::ChannelClosed)),
            }
        }
    }

    /// Download the headers in reverse order (falling block numbers)
    /// starting from the tip of the chain
    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let current_checkpoint = input.checkpoint();

        if self.sync_gap.as_ref().ok_or(StageError::MissingSyncGap)?.is_closed() {
            self.is_etl_ready = false;
            return Ok(ExecOutput::done(current_checkpoint))
        }

        // We should be here only after we have downloaded all headers into the disk buffer (ETL).
        if !self.is_etl_ready {
            return Err(StageError::MissingDownloadBuffer)
        }

        // Reset flag
        self.is_etl_ready = false;

        // Write the headers and related tables to DB from ETL space
        let to_be_processed = self.hash_collector.len() as u64;
        let last_header_number =
            self.write_headers(provider, provider.static_file_provider().clone())?;

        // Clear ETL collectors
        self.hash_collector.clear();
        self.header_collector.clear();

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(last_header_number).with_headers_stage_checkpoint(
                HeadersCheckpoint {
                    block_range: CheckpointBlockRange {
                        from: input.checkpoint().block_number,
                        to: last_header_number,
                    },
                    progress: EntitiesCheckpoint {
                        processed: input.checkpoint().block_number + to_be_processed,
                        total: last_header_number,
                    },
                },
            ),
            // We only reach here if all headers have been downloaded by ETL, and pushed to DB all
            // in one stage run.
            done: true,
        })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.sync_gap.take();

        // First unwind the db tables, until the unwind_to block number. use the walker to unwind
        // HeaderNumbers based on the index in CanonicalHeaders
        provider.unwind_table_by_walker::<tables::CanonicalHeaders, tables::HeaderNumbers>(
            input.unwind_to..,
        )?;
        provider.unwind_table_by_num::<tables::CanonicalHeaders>(input.unwind_to)?;
        provider.unwind_table_by_num::<tables::HeaderTerminalDifficulties>(input.unwind_to)?;
        let unfinalized_headers_unwound =
            provider.unwind_table_by_num::<tables::Headers>(input.unwind_to)?;

        // determine how many headers to unwind from the static files based on the highest block and
        // the unwind_to block
        let static_file_provider = provider.static_file_provider();
        let highest_block = static_file_provider
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .unwrap_or_default();
        let static_file_headers_to_unwind = highest_block - input.unwind_to;
        for block_number in (input.unwind_to + 1)..=highest_block {
            let hash = static_file_provider.block_hash(block_number)?;
            // we have to delete from HeaderNumbers here as well as in the above unwind, since that
            // mapping contains entries for both headers in the db and headers in static files
            //
            // so if we are unwinding past the lowest block in the db, we have to iterate through
            // the HeaderNumbers entries that we'll delete in static files below
            if let Some(header_hash) = hash {
                provider.tx_ref().delete::<tables::HeaderNumbers>(header_hash, None)?;
            }
        }

        // Now unwind the static files until the unwind_to block number
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;
        writer.prune_headers(static_file_headers_to_unwind)?;

        // Set the stage checkpoint entities processed based on how much we unwound - we add the
        // headers unwound from static files and db
        let stage_checkpoint =
            input.checkpoint.headers_stage_checkpoint().map(|stage_checkpoint| HeadersCheckpoint {
                block_range: stage_checkpoint.block_range,
                progress: EntitiesCheckpoint {
                    processed: stage_checkpoint.progress.processed.saturating_sub(
                        static_file_headers_to_unwind + unfinalized_headers_unwound as u64,
                    ),
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
    use reth_execution_types::ExecutionOutcome;
    use reth_primitives::{BlockBody, SealedBlock, SealedBlockWithSenders, B256};
    use reth_provider::{BlockWriter, ProviderFactory, StaticFileProviderFactory};
    use reth_stages_api::StageUnitCheckpoint;
    use reth_testing_utils::generators::{self, random_header, random_header_range};
    use reth_trie::{updates::TrieUpdates, HashedPostStateSorted};
    use test_runner::HeadersTestRunner;

    mod test_runner {
        use super::*;
        use crate::test_utils::{TestRunnerError, TestStageDB};
        use reth_consensus::test_utils::TestConsensus;
        use reth_db::{test_utils::TempDatabase, DatabaseEnv};
        use reth_downloaders::headers::reverse_headers::{
            ReverseHeadersDownloader, ReverseHeadersDownloaderBuilder,
        };
        use reth_network_p2p::test_utils::{TestHeaderDownloader, TestHeadersClient};
        use reth_provider::BlockNumReader;
        use tokio::sync::watch;

        pub(crate) struct HeadersTestRunner<D: HeaderDownloader> {
            pub(crate) client: TestHeadersClient,
            channel: (watch::Sender<B256>, watch::Receiver<B256>),
            downloader_factory: Box<dyn Fn() -> D + Send + Sync + 'static>,
            db: TestStageDB,
            consensus: Arc<TestConsensus>,
        }

        impl Default for HeadersTestRunner<TestHeaderDownloader> {
            fn default() -> Self {
                let client = TestHeadersClient::default();
                Self {
                    client: client.clone(),
                    channel: watch::channel(B256::ZERO),
                    consensus: Arc::new(TestConsensus::default()),
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
                    self.channel.1.clone(),
                    self.consensus.clone(),
                    EtlConfig::default(),
                )
            }
        }

        impl<D: HeaderDownloader + 'static> ExecuteStageTestRunner for HeadersTestRunner<D> {
            type Seed = Vec<SealedHeader>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let mut rng = generators::rng();
                let start = input.checkpoint().block_number;
                let headers = random_header_range(&mut rng, 0..start + 1, B256::ZERO);
                let head = headers.last().cloned().unwrap();
                self.db.insert_headers_with_td(headers.iter())?;

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
                        let mut td = provider
                            .header_td_by_number(initial_checkpoint.saturating_sub(1))?
                            .unwrap_or_default();

                        for block_num in initial_checkpoint..output.checkpoint.block_number {
                            // look up the header hash
                            let hash = provider.block_hash(block_num)?.expect("no header hash");

                            // validate the header number
                            assert_eq!(provider.block_number(hash)?, Some(block_num));

                            // validate the header
                            let header = provider.header_by_number(block_num)?;
                            assert!(header.is_some());
                            let header = header.unwrap().seal_slow();
                            assert_eq!(header.hash(), hash);

                            // validate the header total difficulty
                            td += header.difficulty;
                            assert_eq!(
                                provider.header_td_by_number(block_num)?.map(Into::into),
                                Some(td)
                            );
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
                    consensus: Arc::new(TestConsensus::default()),
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
                self.db.ensure_no_entry_above::<tables::HeaderTerminalDifficulties, _>(
                    block,
                    |num| num,
                )?;
                Ok(())
            }

            pub(crate) fn send_tip(&self, tip: B256) {
                self.channel.0.send(tip).expect("failed to send tip");
            }
        }
    }

    stage_test_suite!(HeadersTestRunner, headers);

    /// Execute the stage with linear downloader, unwinds, and ensures that the database tables
    /// along with the static files are cleaned up.
    #[tokio::test]
    async fn execute_with_linear_downloader_unwind() {
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
        runner.db().factory.static_file_provider().commit().unwrap();
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
            processed == checkpoint + headers.len() as u64 - 1 && total == tip.number
        );
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
        assert!(runner.stage().hash_collector.is_empty());
        assert!(runner.stage().header_collector.is_empty());

        // let's insert some blocks using append_blocks_with_state
        let sealed_headers =
            random_header_range(&mut generators::rng(), tip.number..tip.number + 10, tip.hash());

        // make them sealed blocks with senders by converting them to empty blocks
        let sealed_blocks = sealed_headers
            .iter()
            .map(|header| {
                SealedBlockWithSenders::new(
                    SealedBlock::new(header.clone(), BlockBody::default()),
                    vec![],
                )
                .unwrap()
            })
            .collect();

        // append the blocks
        let provider = runner.db().factory.provider_rw().unwrap();
        provider
            .append_blocks_with_state(
                sealed_blocks,
                ExecutionOutcome::default(),
                HashedPostStateSorted::default(),
                TrieUpdates::default(),
            )
            .unwrap();
        provider.commit().unwrap();

        // now we can unwind 10 blocks
        let unwind_input = UnwindInput {
            checkpoint: StageCheckpoint::new(tip.number + 10),
            unwind_to: tip.number,
            bad_block: None,
        };

        let unwind_output = runner.unwind(unwind_input).await.unwrap();
        assert_eq!(unwind_output.checkpoint.block_number, tip.number);

        // validate the unwind, ensure that the tables are cleaned up
        assert!(runner.validate_unwind(unwind_input).is_ok());
    }

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
        runner.db().factory.static_file_provider().commit().unwrap();
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
            processed == checkpoint + headers.len() as u64 - 1 && total == tip.number
        );
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
        assert!(runner.stage().hash_collector.is_empty());
        assert!(runner.stage().header_collector.is_empty());
    }
}
