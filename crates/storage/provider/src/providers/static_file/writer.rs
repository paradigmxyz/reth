use crate::providers::static_file::metrics::StaticFileProviderOperation;

use super::{
    manager::StaticFileProviderInner, metrics::StaticFileProviderMetrics, StaticFileProvider,
};
use dashmap::mapref::one::RefMut;
use reth_codecs::Compact;
use reth_db::codecs::CompactU256;
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_nippy_jar::{NippyJar, NippyJarError, NippyJarWriter};
use reth_primitives::{
    static_file::{find_fixed_range, SegmentHeader, SegmentRangeInclusive},
    BlockHash, BlockNumber, Header, Receipt, StaticFileSegment, TransactionSignedNoHash, TxNumber,
    U256,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Instant,
};
use tracing::debug;

/// Mutable reference to a dashmap element of [`StaticFileProviderRW`].
pub type StaticFileProviderRWRefMut<'a> = RefMut<'a, StaticFileSegment, StaticFileProviderRW>;

#[derive(Debug)]
/// Extends `StaticFileProvider` with writing capabilities
pub struct StaticFileProviderRW {
    /// Reference back to the provider. We need [Weak] here because [StaticFileProviderRW] is
    /// stored in a [dashmap::DashMap] inside the parent [StaticFileProvider].which is an [Arc].
    /// If we were to use an [Arc] here, we would create a reference cycle.
    reader: Weak<StaticFileProviderInner>,
    writer: NippyJarWriter<SegmentHeader>,
    data_path: PathBuf,
    buf: Vec<u8>,
    metrics: Option<Arc<StaticFileProviderMetrics>>,
}

impl StaticFileProviderRW {
    /// Creates a new [`StaticFileProviderRW`] for a [`StaticFileSegment`].
    pub fn new(
        segment: StaticFileSegment,
        block: BlockNumber,
        reader: Weak<StaticFileProviderInner>,
        metrics: Option<Arc<StaticFileProviderMetrics>>,
    ) -> ProviderResult<Self> {
        let (writer, data_path) = Self::open(segment, block, reader.clone(), metrics.clone())?;
        Ok(Self { writer, data_path, buf: Vec::with_capacity(100), reader, metrics })
    }

    fn open(
        segment: StaticFileSegment,
        block: u64,
        reader: Weak<StaticFileProviderInner>,
        metrics: Option<Arc<StaticFileProviderMetrics>>,
    ) -> ProviderResult<(NippyJarWriter<SegmentHeader>, PathBuf)> {
        let start = Instant::now();

        let static_file_provider = Self::upgrade_provider_to_strong_reference(&reader);

        let block_range = find_fixed_range(block);
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

    /// Commits configuration changes to disk and updates the reader index with the new changes.
    pub fn commit(&mut self) -> ProviderResult<()> {
        let start = Instant::now();

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
        let segment_max_block = match self.writer.user_header().block_range() {
            Some(block_range) => Some(block_range.end()),
            None => {
                if self.writer.user_header().expected_block_start() > 0 {
                    Some(self.writer.user_header().expected_block_start() - 1)
                } else {
                    None
                }
            }
        };

        self.reader().update_index(self.writer.user_header().segment(), segment_max_block)
    }

    /// Allows to increment the [`SegmentHeader`] end block. It will commit the current static file,
    /// and create the next one if we are past the end range.
    ///
    /// Returns the current [`BlockNumber`] as seen in the static file.
    pub fn increment_block(
        &mut self,
        segment: StaticFileSegment,
        expected_block_number: BlockNumber,
    ) -> ProviderResult<BlockNumber> {
        self.check_next_block_number(expected_block_number, segment)?;

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

                *self.writer.user_header_mut() =
                    SegmentHeader::new(find_fixed_range(last_block + 1), None, None, segment);
            }
        }

        let block = self.writer.user_header_mut().increment_block();
        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                segment,
                StaticFileProviderOperation::IncrementBlock,
                Some(start.elapsed()),
            );
        }

        Ok(block)
    }

    /// Verifies if the incoming block number matches the next expected block number
    /// for a static file. This ensures data continuity when adding new blocks.
    fn check_next_block_number(
        &mut self,
        expected_block_number: u64,
        segment: StaticFileSegment,
    ) -> ProviderResult<()> {
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
                segment,
                expected_block_number,
                next_static_file_block,
            ))
        }
        Ok(())
    }

    /// Truncates a number of rows from disk. It deletes and loads an older static file if block
    /// goes beyond the start of the current block range.
    ///
    /// **last_block** should be passed only with transaction based segments.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    fn truncate(
        &mut self,
        segment: StaticFileSegment,
        mut num_rows: u64,
        last_block: Option<u64>,
    ) -> ProviderResult<()> {
        while num_rows > 0 {
            let len = match segment {
                StaticFileSegment::Headers => {
                    self.writer.user_header().block_len().unwrap_or_default()
                }
                StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
                    self.writer.user_header().tx_len().unwrap_or_default()
                }
            };

            if num_rows >= len {
                // If there's more rows to delete than this static file contains, then just
                // delete the whole file and go to the next static file
                let previous_snap = self.data_path.clone();
                let block_start = self.writer.user_header().expected_block_start();

                if block_start != 0 {
                    let (writer, data_path) = Self::open(
                        segment,
                        self.writer.user_header().expected_block_start() - 1,
                        self.reader.clone(),
                        self.metrics.clone(),
                    )?;
                    self.writer = writer;
                    self.data_path = data_path;

                    NippyJar::<SegmentHeader>::load(&previous_snap)
                        .map_err(|e| ProviderError::NippyJar(e.to_string()))?
                        .delete()
                        .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
                } else {
                    // Update `SegmentHeader`
                    self.writer.user_header_mut().prune(len);
                    self.writer
                        .prune_rows(len as usize)
                        .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
                    break
                }

                num_rows -= len;
            } else {
                // Update `SegmentHeader`
                self.writer.user_header_mut().prune(num_rows);

                // Truncate data
                self.writer
                    .prune_rows(num_rows as usize)
                    .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
                num_rows = 0;
            }
        }

        // Only Transactions and Receipts
        if let Some(last_block) = last_block {
            let header = self.writer.user_header_mut();
            header.set_block_range(header.expected_block_start(), last_block);
        }

        // Commits new changes to disk.
        self.commit()?;

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
        segment: StaticFileSegment,
        tx_num: TxNumber,
        value: V,
    ) -> ProviderResult<TxNumber> {
        debug_assert!(self.writer.user_header().segment() == segment);

        if self.writer.user_header().tx_range().is_none() {
            self.writer.user_header_mut().set_tx_range(tx_num, tx_num);
        } else {
            self.writer.user_header_mut().increment_tx();
        }

        self.append_column(value)?;

        Ok(self.writer.user_header().tx_end().expect("qed"))
    }

    /// Appends header to static file.
    ///
    /// It **CALLS** `increment_block()` since the number of headers is equal to the number of
    /// blocks.
    ///
    /// Returns the current [`BlockNumber`] as seen in the static file.
    pub fn append_header(
        &mut self,
        header: Header,
        terminal_difficulty: U256,
        hash: BlockHash,
    ) -> ProviderResult<BlockNumber> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Headers);

        let block_number = self.increment_block(StaticFileSegment::Headers, header.number)?;

        self.append_column(header)?;
        self.append_column(CompactU256::from(terminal_difficulty))?;
        self.append_column(hash)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Headers,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(block_number)
    }

    /// Appends transaction to static file.
    ///
    /// It **DOES NOT CALL** `increment_block()`, it should be handled elsewhere. There might be
    /// empty blocks and this function wouldn't be called.
    ///
    /// Returns the current [`TxNumber`] as seen in the static file.
    pub fn append_transaction(
        &mut self,
        tx_num: TxNumber,
        tx: TransactionSignedNoHash,
    ) -> ProviderResult<TxNumber> {
        let start = Instant::now();

        let result = self.append_with_tx_number(StaticFileSegment::Transactions, tx_num, tx)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Transactions,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(result)
    }

    /// Appends receipt to static file.
    ///
    /// It **DOES NOT** call `increment_block()`, it should be handled elsewhere. There might be
    /// empty blocks and this function wouldn't be called.
    ///
    /// Returns the current [`TxNumber`] as seen in the static file.
    pub fn append_receipt(
        &mut self,
        tx_num: TxNumber,
        receipt: Receipt,
    ) -> ProviderResult<TxNumber> {
        let start = Instant::now();

        let result = self.append_with_tx_number(StaticFileSegment::Receipts, tx_num, receipt)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(result)
    }

    /// Removes the last `number` of transactions from static files.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    pub fn prune_transactions(
        &mut self,
        number: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let start = Instant::now();

        let segment = StaticFileSegment::Transactions;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, number, Some(last_block))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Transactions,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes `to_delete` number of receipts from static_files.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    pub fn prune_receipts(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let start = Instant::now();

        let segment = StaticFileSegment::Receipts;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, to_delete, Some(last_block))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes `to_delete` number of headers from static_files.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    pub fn prune_headers(&mut self, to_delete: u64) -> ProviderResult<()> {
        let start = Instant::now();

        let segment = StaticFileSegment::Headers;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, to_delete, None)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Headers,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    fn reader(&self) -> StaticFileProvider {
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
        provider: &Weak<StaticFileProviderInner>,
    ) -> StaticFileProvider {
        provider.upgrade().map(StaticFileProvider).expect("StaticFileProvider is dropped")
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Helper function to override block range for testing.
    pub fn set_block_range(&mut self, block_range: std::ops::RangeInclusive<BlockNumber>) {
        self.writer.user_header_mut().set_block_range(*block_range.start(), *block_range.end())
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Helper function to access [`SegmentHeader`].
    pub fn user_header(&self) -> &SegmentHeader {
        self.writer.user_header()
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
