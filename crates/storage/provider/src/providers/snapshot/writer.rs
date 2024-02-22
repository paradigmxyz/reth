use super::SnapshotProvider;
use dashmap::mapref::one::RefMut;
use reth_codecs::Compact;
use reth_db::codecs::CompactU256;
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_nippy_jar::{NippyJar, NippyJarError, NippyJarWriter};
use reth_primitives::{
    snapshot::{find_fixed_range, SegmentHeader},
    BlockHash, BlockNumber, Header, Receipt, SnapshotSegment, TransactionSignedNoHash, TxNumber,
    U256,
};
use std::{
    ops::Deref,
    path::{Path, PathBuf},
    time::Instant,
};
use tracing::debug;

/// Mutable reference to a dashmap element of [`SnapshotProviderRW`].
pub type SnapshotProviderRWRefMut<'a> = RefMut<'a, SnapshotSegment, SnapshotProviderRW<'static>>;

#[derive(Debug)]
/// Extends `SnapshotProvider` with writing capabilities
pub struct SnapshotProviderRW<'a> {
    reader: SnapshotProvider,
    writer: NippyJarWriter<'a, SegmentHeader>,
    data_path: PathBuf,
    buf: Vec<u8>,
}

impl<'a> SnapshotProviderRW<'a> {
    /// Creates a new [`SnapshotProviderRW`] for a [`SnapshotSegment`].
    pub fn new(
        segment: SnapshotSegment,
        block: BlockNumber,
        reader: SnapshotProvider,
    ) -> ProviderResult<Self> {
        let (writer, data_path) = Self::open(segment, block, reader.clone())?;
        Ok(Self { writer, data_path, buf: Vec::with_capacity(100), reader })
    }

    fn open(
        segment: SnapshotSegment,
        block: u64,
        reader: SnapshotProvider,
    ) -> ProviderResult<(NippyJarWriter<'a, SegmentHeader>, PathBuf)> {
        let block_range = find_fixed_range(block);
        let (jar, path) =
            match reader.get_segment_provider_from_block(segment, block_range.start(), None) {
                Ok(provider) => {
                    (NippyJar::load(provider.data_path())?, provider.data_path().into())
                }
                Err(ProviderError::MissingSnapshotBlock(_, _)) => {
                    let path = reader.directory().join(segment.filename(&block_range));
                    (create_jar(segment, &path, block_range.start()), path)
                }
                Err(err) => return Err(err),
            };

        match NippyJarWriter::from_owned(jar) {
            Ok(writer) => Ok((writer, path)),
            Err(NippyJarError::FrozenJar) => {
                // This snapshot has been frozen, so we should
                Err(ProviderError::FinalizedSnapshot(segment, block))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Commits configuration changes to disk and updates the reader index with the new changes.
    pub fn commit(&mut self) -> ProviderResult<()> {
        let time = Instant::now();

        // Commits offsets and new user_header to disk
        self.writer.commit()?;

        debug!(
            target: "provider::static_file",
            segment = ?self.writer.user_header().segment(),
            path = ?self.data_path,
            duration = ?time.elapsed(),
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
        // substracting 1 from the expected block start, resulting on the last block of the
        // previous file.
        //
        // If that expected block start is 0, then it means that there's no actual block data, and
        // there's no snapshotted block data.
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

        self.reader.update_index(self.writer.user_header().segment(), segment_max_block)
    }

    /// Allows to increment the [`SegmentHeader`] end block. It will commit the current snapshot,
    /// and create the next one if we are past the end range.
    ///
    /// Returns the current [`BlockNumber`] as seen in the static file.
    pub fn increment_block(&mut self, segment: SnapshotSegment) -> ProviderResult<BlockNumber> {
        if let Some(last_block) = self.writer.user_header().block_end() {
            let writer_range_end = find_fixed_range(last_block).end();

            // We have finished the previous snapshot and must freeze it
            if last_block + 1 > writer_range_end {
                // Commits offsets and new user_header to disk
                self.commit()?;

                // Opens the new snapshot
                let (writer, data_path) = Self::open(segment, last_block + 1, self.reader.clone())?;
                self.writer = writer;
                self.data_path = data_path;

                let block_start = find_fixed_range(last_block + 1).start();
                *self.writer.user_header_mut() =
                    SegmentHeader::new(block_start, None, None, segment);
            }
        }

        Ok(self.writer.user_header_mut().increment_block())
    }

    /// Truncates a number of rows from disk. It deletes and loads an older snapshot file if block
    /// goes beyond the start of the current block range.
    ///
    /// **last_block** should be passed only with transaction based segments.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    fn truncate(
        &mut self,
        segment: SnapshotSegment,
        mut num_rows: u64,
        last_block: Option<u64>,
    ) -> ProviderResult<()> {
        while num_rows > 0 {
            let len = match segment {
                SnapshotSegment::Headers => {
                    self.writer.user_header().block_len().unwrap_or_default()
                }
                SnapshotSegment::Transactions | SnapshotSegment::Receipts => {
                    self.writer.user_header().tx_len().unwrap_or_default()
                }
            };

            if num_rows >= len {
                // If there's more rows to delete than this snapshot contains, then just
                // delete the whole file and go to the next snapshot
                let previous_snap = self.data_path.clone();
                let block_start = self.writer.user_header().expected_block_start();

                if block_start != 0 {
                    let (writer, data_path) = Self::open(
                        segment,
                        self.writer.user_header().expected_block_start() - 1,
                        self.reader.clone(),
                    )?;
                    self.writer = writer;
                    self.data_path = data_path;

                    NippyJar::<SegmentHeader>::load(&previous_snap)?.delete()?;
                } else {
                    // Update `SegmentHeader`
                    self.writer.user_header_mut().prune(len);
                    self.writer.prune_rows(len as usize)?;
                    break
                }

                num_rows -= len;
            } else {
                // Update `SegmentHeader`
                self.writer.user_header_mut().prune(num_rows);

                // Truncate data
                self.writer.prune_rows(num_rows as usize)?;
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

    /// Appends column to snapshot file.
    fn append_column<T: Compact>(&mut self, column: T) -> ProviderResult<()> {
        self.buf.clear();
        column.to_compact(&mut self.buf);

        self.writer.append_column(Some(Ok(&self.buf)))?;
        Ok(())
    }

    /// Appends to tx number-based snapshot file.
    ///
    /// Returns the current [`TxNumber`] as seen in the static file.
    fn append_with_tx_number<V: Compact>(
        &mut self,
        segment: SnapshotSegment,
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

        Ok(self.writer.user_header().tx_end())
    }

    /// Appends header to snapshot file.
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
        debug_assert!(self.writer.user_header().segment() == SnapshotSegment::Headers);

        let block_number = self.increment_block(SnapshotSegment::Headers)?;

        self.append_column(header)?;
        self.append_column(CompactU256::from(terminal_difficulty))?;
        self.append_column(hash)?;

        Ok(block_number)
    }

    /// Appends transaction to snapshot file.
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
        self.append_with_tx_number(SnapshotSegment::Transactions, tx_num, tx)
    }

    /// Appends receipt to snapshot file.
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
        self.append_with_tx_number(SnapshotSegment::Receipts, tx_num, receipt)
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
        let segment = SnapshotSegment::Transactions;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, number, Some(last_block))
    }

    /// Prunes `to_delete` number of receipts from snapshots.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    pub fn prune_receipts(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let segment = SnapshotSegment::Receipts;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, to_delete, Some(last_block))
    }

    /// Prunes `to_delete` number of headers from snapshots.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    pub fn prune_headers(&mut self, to_delete: u64) -> ProviderResult<()> {
        let segment = SnapshotSegment::Headers;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, to_delete, None)
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Helper function to override block range for testing.
    pub fn set_block_range(&mut self, block_range: std::ops::RangeInclusive<BlockNumber>) {
        self.writer.user_header_mut().set_block_range(*block_range.start(), *block_range.end())
    }
}

impl<'a> Deref for SnapshotProviderRW<'a> {
    type Target = SnapshotProvider;
    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

fn create_jar(
    segment: SnapshotSegment,
    path: &Path,
    expected_block_start: BlockNumber,
) -> NippyJar<SegmentHeader> {
    let mut jar = NippyJar::new(
        segment.columns(),
        path,
        SegmentHeader::new(expected_block_start, None, None, segment),
    );

    // Transaction and Receipt already have the compression scheme used natively in its encoding.
    // (zstd-dictionary)
    if segment.is_headers() {
        jar = jar.with_lz4();
    }

    jar
}
