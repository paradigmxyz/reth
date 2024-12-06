use super::{
    manager::StaticFileProviderInner, metrics::StaticFileProviderMetrics, StaticFileProvider,
};
use crate::providers::static_file::metrics::StaticFileProviderOperation;
use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockHash, BlockNumber, TxNumber, U256};
use parking_lot::{lock_api::RwLockWriteGuard, RawRwLock, RwLock};
use reth_codecs::Compact;
use reth_db_api::models::CompactU256;
use reth_nippy_jar::{NippyJar, NippyJarError, NippyJarWriter};
use reth_node_types::NodePrimitives;
use reth_primitives::{
    static_file::{SegmentHeader, SegmentRangeInclusive},
    Receipt, StaticFileSegment,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::{
    borrow::Borrow,
    fmt::Debug,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Instant,
};
use tracing::debug;

/// Static file writers for every known [`StaticFileSegment`].
///
/// WARNING: Trying to use more than one writer for the same segment type **will result in a
/// deadlock**.
#[derive(Debug)]
pub(crate) struct StaticFileWriters<N> {
    headers: RwLock<Option<StaticFileProviderRW<N>>>,
    transactions: RwLock<Option<StaticFileProviderRW<N>>>,
    receipts: RwLock<Option<StaticFileProviderRW<N>>>,
}

impl<N> Default for StaticFileWriters<N> {
    fn default() -> Self {
        Self {
            headers: Default::default(),
            transactions: Default::default(),
            receipts: Default::default(),
        }
    }
}

impl<N: NodePrimitives> StaticFileWriters<N> {
    pub(crate) fn get_or_create(
        &self,
        segment: StaticFileSegment,
        create_fn: impl FnOnce() -> ProviderResult<StaticFileProviderRW<N>>,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_, N>> {
        let mut write_guard = match segment {
            StaticFileSegment::Headers => self.headers.write(),
            StaticFileSegment::Transactions => self.transactions.write(),
            StaticFileSegment::Receipts => self.receipts.write(),
        };

        if write_guard.is_none() {
            *write_guard = Some(create_fn()?);
        }

        Ok(StaticFileProviderRWRefMut(write_guard))
    }

    pub(crate) fn commit(&self) -> ProviderResult<()> {
        for writer_lock in [&self.headers, &self.transactions, &self.receipts] {
            let mut writer = writer_lock.write();
            if let Some(writer) = writer.as_mut() {
                writer.commit()?;
            }
        }
        Ok(())
    }
}

/// Mutable reference to a [`StaticFileProviderRW`] behind a [`RwLockWriteGuard`].
#[derive(Debug)]
pub struct StaticFileProviderRWRefMut<'a, N>(
    pub(crate) RwLockWriteGuard<'a, RawRwLock, Option<StaticFileProviderRW<N>>>,
);

impl<N> std::ops::DerefMut for StaticFileProviderRWRefMut<'_, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // This is always created by [`StaticFileWriters::get_or_create`]
        self.0.as_mut().expect("static file writer provider should be init")
    }
}

impl<N> std::ops::Deref for StaticFileProviderRWRefMut<'_, N> {
    type Target = StaticFileProviderRW<N>;

    fn deref(&self) -> &Self::Target {
        // This is always created by [`StaticFileWriters::get_or_create`]
        self.0.as_ref().expect("static file writer provider should be init")
    }
}

#[derive(Debug)]
/// Extends `StaticFileProvider` with writing capabilities
pub struct StaticFileProviderRW<N> {
    /// Reference back to the provider. We need [Weak] here because [`StaticFileProviderRW`] is
    /// stored in a [`dashmap::DashMap`] inside the parent [`StaticFileProvider`].which is an
    /// [Arc]. If we were to use an [Arc] here, we would create a reference cycle.
    reader: Weak<StaticFileProviderInner<N>>,
    /// A [`NippyJarWriter`] instance.
    writer: NippyJarWriter<SegmentHeader>,
    /// Path to opened file.
    data_path: PathBuf,
    /// Reusable buffer for encoding appended data.
    buf: Vec<u8>,
    /// Metrics.
    metrics: Option<Arc<StaticFileProviderMetrics>>,
    /// On commit, does the instructed pruning: number of lines, and if it applies, the last block
    /// it ends at.
    prune_on_commit: Option<(u64, Option<BlockNumber>)>,
}

impl<N: NodePrimitives> StaticFileProviderRW<N> {
    /// Creates a new [`StaticFileProviderRW`] for a [`StaticFileSegment`].
    ///
    /// Before use, transaction based segments should ensure the block end range is the expected
    /// one, and heal if not. For more check `Self::ensure_end_range_consistency`.
    pub fn new(
        segment: StaticFileSegment,
        block: BlockNumber,
        reader: Weak<StaticFileProviderInner<N>>,
        metrics: Option<Arc<StaticFileProviderMetrics>>,
    ) -> ProviderResult<Self> {
        let (writer, data_path) = Self::open(segment, block, reader.clone(), metrics.clone())?;
        let mut writer = Self {
            writer,
            data_path,
            buf: Vec::with_capacity(100),
            reader,
            metrics,
            prune_on_commit: None,
        };

        writer.ensure_end_range_consistency()?;

        Ok(writer)
    }

    fn open(
        segment: StaticFileSegment,
        block: u64,
        reader: Weak<StaticFileProviderInner<N>>,
        metrics: Option<Arc<StaticFileProviderMetrics>>,
    ) -> ProviderResult<(NippyJarWriter<SegmentHeader>, PathBuf)> {
        let start = Instant::now();

        let static_file_provider = Self::upgrade_provider_to_strong_reference(&reader);

        let block_range = static_file_provider.find_fixed_range(block);
        let (jar, path) = match static_file_provider.get_segment_provider_from_block(
            segment,
            block_range.start(),
            None,
        ) {
            Ok(provider) => (
                NippyJar::load(provider.data_path())
                    .map_err(|e| ProviderError::NippyJar(e.to_string()))?,
                provider.data_path().into(),
            ),
            Err(ProviderError::MissingStaticFileBlock(_, _)) => {
                let path = static_file_provider.directory().join(segment.filename(&block_range));
                (create_jar(segment, &path, block_range), path)
            }
            Err(err) => return Err(err),
        };

        let result = match NippyJarWriter::new(jar) {
            Ok(writer) => Ok((writer, path)),
            Err(NippyJarError::FrozenJar) => {
                // This static file has been frozen, so we should
                Err(ProviderError::FinalizedStaticFile(segment, block))
            }
            Err(e) => Err(ProviderError::NippyJar(e.to_string())),
        }?;

        if let Some(metrics) = &metrics {
            metrics.record_segment_operation(
                segment,
                StaticFileProviderOperation::OpenWriter,
                Some(start.elapsed()),
            );
        }

        Ok(result)
    }

    /// If a file level healing happens, we need to update the end range on the
    /// [`SegmentHeader`].
    ///
    /// However, for transaction based segments, the block end range has to be found and healed
    /// externally.
    ///
    /// Check [`reth_nippy_jar::NippyJarChecker`] &
    /// [`NippyJarWriter`] for more on healing.
    fn ensure_end_range_consistency(&mut self) -> ProviderResult<()> {
        // If we have lost rows (in this run or previous), we need to update the [SegmentHeader].
        let expected_rows = if self.user_header().segment().is_headers() {
            self.user_header().block_len().unwrap_or_default()
        } else {
            self.user_header().tx_len().unwrap_or_default()
        };
        let pruned_rows = expected_rows - self.writer.rows() as u64;
        if pruned_rows > 0 {
            self.user_header_mut().prune(pruned_rows);
        }

        self.writer.commit().map_err(|error| ProviderError::NippyJar(error.to_string()))?;

        // Updates the [SnapshotProvider] manager
        self.update_index()?;
        Ok(())
    }

    /// Commits configuration changes to disk and updates the reader index with the new changes.
    pub fn commit(&mut self) -> ProviderResult<()> {
        let start = Instant::now();

        // Truncates the data file if instructed to.
        if let Some((to_delete, last_block_number)) = self.prune_on_commit.take() {
            match self.writer.user_header().segment() {
                StaticFileSegment::Headers => self.prune_header_data(to_delete)?,
                StaticFileSegment::Transactions => self
                    .prune_transaction_data(to_delete, last_block_number.expect("should exist"))?,
                StaticFileSegment::Receipts => {
                    self.prune_receipt_data(to_delete, last_block_number.expect("should exist"))?
                }
            }
        }

        if self.writer.is_dirty() {
            // Commits offsets and new user_header to disk
            self.writer.commit().map_err(|e| ProviderError::NippyJar(e.to_string()))?;

            if let Some(metrics) = &self.metrics {
                metrics.record_segment_operation(
                    self.writer.user_header().segment(),
                    StaticFileProviderOperation::CommitWriter,
                    Some(start.elapsed()),
                );
            }

            debug!(
                target: "provider::static_file",
                segment = ?self.writer.user_header().segment(),
                path = ?self.data_path,
                duration = ?start.elapsed(),
                "Commit"
            );

            self.update_index()?;
        }

        Ok(())
    }

    /// Commits configuration changes to disk and updates the reader index with the new changes.
    ///
    /// CAUTION: does not call `sync_all` on the files.
    #[cfg(feature = "test-utils")]
    pub fn commit_without_sync_all(&mut self) -> ProviderResult<()> {
        let start = Instant::now();

        // Commits offsets and new user_header to disk
        self.writer
            .commit_without_sync_all()
            .map_err(|e| ProviderError::NippyJar(e.to_string()))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                self.writer.user_header().segment(),
                StaticFileProviderOperation::CommitWriter,
                Some(start.elapsed()),
            );
        }

        debug!(
            target: "provider::static_file",
            segment = ?self.writer.user_header().segment(),
            path = ?self.data_path,
            duration = ?start.elapsed(),
            "Commit"
        );

        self.update_index()?;

        Ok(())
    }

    /// Updates the `self.reader` internal index.
    fn update_index(&self) -> ProviderResult<()> {
        // We find the maximum block of the segment by checking this writer's last block.
        //
        // However if there's no block range (because there's no data), we try to calculate it by
        // subtracting 1 from the expected block start, resulting on the last block of the
        // previous file.
        //
        // If that expected block start is 0, then it means that there's no actual block data, and
        // there's no block data in static files.
        let segment_max_block = self
            .writer
            .user_header()
            .block_range()
            .as_ref()
            .map(|block_range| block_range.end())
            .or_else(|| {
                (self.writer.user_header().expected_block_start() > 0)
                    .then(|| self.writer.user_header().expected_block_start() - 1)
            });

        self.reader().update_index(self.writer.user_header().segment(), segment_max_block)
    }

    /// Allows to increment the [`SegmentHeader`] end block. It will commit the current static file,
    /// and create the next one if we are past the end range.
    ///
    /// Returns the current [`BlockNumber`] as seen in the static file.
    pub fn increment_block(&mut self, expected_block_number: BlockNumber) -> ProviderResult<()> {
        let segment = self.writer.user_header().segment();

        self.check_next_block_number(expected_block_number)?;

        let start = Instant::now();
        if let Some(last_block) = self.writer.user_header().block_end() {
            // We have finished the previous static file and must freeze it
            if last_block == self.writer.user_header().expected_block_end() {
                // Commits offsets and new user_header to disk
                self.commit()?;

                // Opens the new static file
                let (writer, data_path) =
                    Self::open(segment, last_block + 1, self.reader.clone(), self.metrics.clone())?;
                self.writer = writer;
                self.data_path = data_path;

                *self.writer.user_header_mut() = SegmentHeader::new(
                    self.reader().find_fixed_range(last_block + 1),
                    None,
                    None,
                    segment,
                );
            }
        }

        self.writer.user_header_mut().increment_block();
        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                segment,
                StaticFileProviderOperation::IncrementBlock,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Verifies if the incoming block number matches the next expected block number
    /// for a static file. This ensures data continuity when adding new blocks.
    fn check_next_block_number(&self, expected_block_number: u64) -> ProviderResult<()> {
        // The next static file block number can be found by checking the one after block_end.
        // However if it's a new file that hasn't been added any data, its block range will actually
        // be None. In that case, the next block will be found on `expected_block_start`.
        let next_static_file_block = self
            .writer
            .user_header()
            .block_end()
            .map(|b| b + 1)
            .unwrap_or_else(|| self.writer.user_header().expected_block_start());

        if expected_block_number != next_static_file_block {
            return Err(ProviderError::UnexpectedStaticFileBlockNumber(
                self.writer.user_header().segment(),
                expected_block_number,
                next_static_file_block,
            ))
        }
        Ok(())
    }

    /// Truncates a number of rows from disk. It deletes and loads an older static file if block
    /// goes beyond the start of the current block range.
    ///
    /// **`last_block`** should be passed only with transaction based segments.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    fn truncate(&mut self, num_rows: u64, last_block: Option<u64>) -> ProviderResult<()> {
        let mut remaining_rows = num_rows;
        let segment = self.writer.user_header().segment();
        while remaining_rows > 0 {
            let len = match segment {
                StaticFileSegment::Headers => {
                    self.writer.user_header().block_len().unwrap_or_default()
                }
                StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
                    self.writer.user_header().tx_len().unwrap_or_default()
                }
            };

            if remaining_rows >= len {
                // If there's more rows to delete than this static file contains, then just
                // delete the whole file and go to the next static file
                let block_start = self.writer.user_header().expected_block_start();

                // We only delete the file if it's NOT the first static file AND:
                // * it's a Header segment  OR
                // * it's a tx-based segment AND `last_block` is lower than the first block of this
                //   file's block range. Otherwise, having no rows simply means that this block
                //   range has no transactions, but the file should remain.
                if block_start != 0 &&
                    (segment.is_headers() || last_block.is_some_and(|b| b < block_start))
                {
                    self.delete_current_and_open_previous()?;
                } else {
                    // Update `SegmentHeader`
                    self.writer.user_header_mut().prune(len);
                    self.writer
                        .prune_rows(len as usize)
                        .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
                    break
                }

                remaining_rows -= len;
            } else {
                // Update `SegmentHeader`
                self.writer.user_header_mut().prune(remaining_rows);

                // Truncate data
                self.writer
                    .prune_rows(remaining_rows as usize)
                    .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
                remaining_rows = 0;
            }
        }

        // Only Transactions and Receipts
        if let Some(last_block) = last_block {
            let mut expected_block_start = self.writer.user_header().expected_block_start();

            if num_rows == 0 {
                // Edge case for when we are unwinding a chain of empty blocks that goes across
                // files, and therefore, the only reference point to know which file
                // we are supposed to be at is `last_block`.
                while last_block < expected_block_start {
                    self.delete_current_and_open_previous()?;
                    expected_block_start = self.writer.user_header().expected_block_start();
                }
            }
            self.writer.user_header_mut().set_block_range(expected_block_start, last_block);
        }

        // Commits new changes to disk.
        self.commit()?;

        Ok(())
    }

    /// Delete the current static file, and replace this provider writer with the previous static
    /// file.
    fn delete_current_and_open_previous(&mut self) -> Result<(), ProviderError> {
        let current_path = self.data_path.clone();
        let (previous_writer, data_path) = Self::open(
            self.user_header().segment(),
            self.writer.user_header().expected_block_start() - 1,
            self.reader.clone(),
            self.metrics.clone(),
        )?;
        self.writer = previous_writer;
        self.writer.set_dirty();
        self.data_path = data_path;
        NippyJar::<SegmentHeader>::load(&current_path)
            .map_err(|e| ProviderError::NippyJar(e.to_string()))?
            .delete()
            .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
        Ok(())
    }

    /// Appends column to static file.
    fn append_column<T: Compact>(&mut self, column: T) -> ProviderResult<()> {
        self.buf.clear();
        column.to_compact(&mut self.buf);

        self.writer
            .append_column(Some(Ok(&self.buf)))
            .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
        Ok(())
    }

    /// Appends to tx number-based static file.
    ///
    /// Returns the current [`TxNumber`] as seen in the static file.
    fn append_with_tx_number<V: Compact>(
        &mut self,
        tx_num: TxNumber,
        value: V,
    ) -> ProviderResult<()> {
        if let Some(range) = self.writer.user_header().tx_range() {
            let next_tx = range.end() + 1;
            if next_tx != tx_num {
                return Err(ProviderError::UnexpectedStaticFileTxNumber(
                    self.writer.user_header().segment(),
                    tx_num,
                    next_tx,
                ))
            }
            self.writer.user_header_mut().increment_tx();
        } else {
            self.writer.user_header_mut().set_tx_range(tx_num, tx_num);
        }

        self.append_column(value)?;

        Ok(())
    }

    /// Appends header to static file.
    ///
    /// It **CALLS** `increment_block()` since the number of headers is equal to the number of
    /// blocks.
    ///
    /// Returns the current [`BlockNumber`] as seen in the static file.
    pub fn append_header(
        &mut self,
        header: &N::BlockHeader,
        total_difficulty: U256,
        hash: &BlockHash,
    ) -> ProviderResult<()>
    where
        N::BlockHeader: Compact,
    {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Headers);

        self.increment_block(header.number())?;

        self.append_column(header)?;
        self.append_column(CompactU256::from(total_difficulty))?;
        self.append_column(hash)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Headers,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends transaction to static file.
    ///
    /// It **DOES NOT CALL** `increment_block()`, it should be handled elsewhere. There might be
    /// empty blocks and this function wouldn't be called.
    ///
    /// Returns the current [`TxNumber`] as seen in the static file.
    pub fn append_transaction(&mut self, tx_num: TxNumber, tx: &N::SignedTx) -> ProviderResult<()>
    where
        N::SignedTx: Compact,
    {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Transactions);
        self.append_with_tx_number(tx_num, tx)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Transactions,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends receipt to static file.
    ///
    /// It **DOES NOT** call `increment_block()`, it should be handled elsewhere. There might be
    /// empty blocks and this function wouldn't be called.
    ///
    /// Returns the current [`TxNumber`] as seen in the static file.
    pub fn append_receipt(&mut self, tx_num: TxNumber, receipt: &N::Receipt) -> ProviderResult<()>
    where
        N::Receipt: Compact,
    {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Receipts);
        self.append_with_tx_number(tx_num, receipt)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends multiple receipts to the static file.
    ///
    /// Returns the current [`TxNumber`] as seen in the static file, if any.
    pub fn append_receipts<I, R>(&mut self, receipts: I) -> ProviderResult<Option<TxNumber>>
    where
        I: Iterator<Item = Result<(TxNumber, R), ProviderError>>,
        R: Borrow<Receipt>,
    {
        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Receipts);

        let mut receipts_iter = receipts.into_iter().peekable();
        // If receipts are empty, we can simply return None
        if receipts_iter.peek().is_none() {
            return Ok(None);
        }

        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        // At this point receipts contains at least one receipt, so this would be overwritten.
        let mut tx_number = 0;
        let mut count: u64 = 0;

        for receipt_result in receipts_iter {
            let (tx_num, receipt) = receipt_result?;
            self.append_with_tx_number(tx_num, receipt.borrow())?;
            tx_number = tx_num;
            count += 1;
        }

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operations(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Append,
                count,
                Some(start.elapsed()),
            );
        }

        Ok(Some(tx_number))
    }

    /// Adds an instruction to prune `to_delete`transactions during commit.
    ///
    /// Note: `last_block` refers to the block the unwinds ends at.
    pub fn prune_transactions(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::Transactions);
        self.queue_prune(to_delete, Some(last_block))
    }

    /// Adds an instruction to prune `to_delete` receipts during commit.
    ///
    /// Note: `last_block` refers to the block the unwinds ends at.
    pub fn prune_receipts(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::Receipts);
        self.queue_prune(to_delete, Some(last_block))
    }

    /// Adds an instruction to prune `to_delete` headers during commit.
    pub fn prune_headers(&mut self, to_delete: u64) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::Headers);
        self.queue_prune(to_delete, None)
    }

    /// Adds an instruction to prune `to_delete` elements during commit.
    ///
    /// Note: `last_block` refers to the block the unwinds ends at if dealing with transaction-based
    /// data.
    fn queue_prune(
        &mut self,
        to_delete: u64,
        last_block: Option<BlockNumber>,
    ) -> ProviderResult<()> {
        self.ensure_no_queued_prune()?;
        self.prune_on_commit = Some((to_delete, last_block));
        Ok(())
    }

    /// Returns Error if there is a pruning instruction that needs to be applied.
    fn ensure_no_queued_prune(&self) -> ProviderResult<()> {
        if self.prune_on_commit.is_some() {
            return Err(ProviderError::NippyJar(
                "Pruning should be committed before appending or pruning more data".to_string(),
            ))
        }
        Ok(())
    }

    /// Removes the last `to_delete` transactions from the data file.
    fn prune_transaction_data(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Transactions);

        self.truncate(to_delete, Some(last_block))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Transactions,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes the last `to_delete` receipts from the data file.
    fn prune_receipt_data(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Receipts);

        self.truncate(to_delete, Some(last_block))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes the last `to_delete` headers from the data file.
    fn prune_header_data(&mut self, to_delete: u64) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Headers);

        self.truncate(to_delete, None)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Headers,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    fn reader(&self) -> StaticFileProvider<N> {
        Self::upgrade_provider_to_strong_reference(&self.reader)
    }

    /// Upgrades a weak reference of [`StaticFileProviderInner`] to a strong reference
    /// [`StaticFileProvider`].
    ///
    /// # Panics
    ///
    /// Panics if the parent [`StaticFileProvider`] is fully dropped while the child writer is still
    /// active. In reality, it's impossible to detach the [`StaticFileProviderRW`] from the
    /// [`StaticFileProvider`].
    fn upgrade_provider_to_strong_reference(
        provider: &Weak<StaticFileProviderInner<N>>,
    ) -> StaticFileProvider<N> {
        provider.upgrade().map(StaticFileProvider).expect("StaticFileProvider is dropped")
    }

    /// Helper function to access [`SegmentHeader`].
    pub const fn user_header(&self) -> &SegmentHeader {
        self.writer.user_header()
    }

    /// Helper function to access a mutable reference to [`SegmentHeader`].
    pub fn user_header_mut(&mut self) -> &mut SegmentHeader {
        self.writer.user_header_mut()
    }

    /// Helper function to override block range for testing.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_block_range(&mut self, block_range: std::ops::RangeInclusive<BlockNumber>) {
        self.writer.user_header_mut().set_block_range(*block_range.start(), *block_range.end())
    }

    /// Helper function to override block range for testing.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn inner(&mut self) -> &mut NippyJarWriter<SegmentHeader> {
        &mut self.writer
    }
}

fn create_jar(
    segment: StaticFileSegment,
    path: &Path,
    expected_block_range: SegmentRangeInclusive,
) -> NippyJar<SegmentHeader> {
    let mut jar = NippyJar::new(
        segment.columns(),
        path,
        SegmentHeader::new(expected_block_range, None, None, segment),
    );

    // Transaction and Receipt already have the compression scheme used natively in its encoding.
    // (zstd-dictionary)
    if segment.is_headers() {
        jar = jar.with_lz4();
    }

    jar
}
