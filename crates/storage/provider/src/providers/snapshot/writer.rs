use super::SnapshotProvider;
use reth_codecs::Compact;
use reth_db::table::Table;
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_nippy_jar::{NippyJar, NippyJarError, NippyJarWriter};
use reth_primitives::{
    snapshot::SegmentHeader, BlockNumber, Header, Receipt, SnapshotSegment,
    TransactionSignedNoHash, TxNumber,
};
use std::{
    ops::{Deref, RangeInclusive},
    path::PathBuf,
    sync::Arc,
};

const BLOCKS_PER_SNAPSHOT: u64 = 500_000;

#[derive(Debug)]
/// Extends `SnapshotProvider` with writing capabilities
pub struct SnapshotProviderRW<'a> {
    reader: Arc<SnapshotProvider>,
    writer: NippyJarWriter<'a, SegmentHeader>,
    data_path: PathBuf,
    buf: Vec<u8>,
}

impl<'a> SnapshotProviderRW<'a> {
    /// Creates a new [`SnapshotProviderRW`] for a [`SnapshotSegment`].
    pub fn new(
        segment: SnapshotSegment,
        block: BlockNumber,
        reader: Arc<SnapshotProvider>,
    ) -> ProviderResult<Self> {
        let (writer, data_path) = Self::open(segment, block, reader.clone())?;
        Ok(Self { writer, data_path, buf: Vec::with_capacity(100), reader })
    }

    fn open(
        segment: SnapshotSegment,
        block: u64,
        reader: Arc<SnapshotProvider>,
    ) -> ProviderResult<(NippyJarWriter<'a, SegmentHeader>, PathBuf)> {
        let block_range = find_range(BLOCKS_PER_SNAPSHOT, block);
        let (jar, path) = match reader.get_segment_provider_from_block(segment, block, None) {
            Ok(provider) => (NippyJar::load(provider.data_path())?, provider.data_path().into()),
            Err(ProviderError::MissingSnapshotBlock(_, _)) => {
                // TODO(joshie): if its a receipt segment, we can find out the actual range.
                let tx = reader.get_highest_snapshot_tx(segment).unwrap_or(0);
                let tx_range = tx..=tx;
                let path = reader.directory().join(segment.filename(&block_range, &tx_range));

                (
                    NippyJar::new(
                        segment.columns(),
                        &path,
                        SegmentHeader::new(block_range, tx_range, segment),
                    ),
                    path,
                )
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

    /// Commits configuration changes to disk, and updates the filename to reflect the updated block
    /// and/or tx ranges.
    pub fn commit(&mut self) -> ProviderResult<()> {
        let segment = self.writer.user_header().segment();
        let block = self.writer.user_header().block_end();

        // Commits offsets and new user_header to disk
        self.writer.commit()?;

        // Ensures block range and transaction range are flushed to the filename
        let previous_path = self.data_path.clone();
        let new_path = self
            .reader
            .directory()
            .join(segment.filename_from_header(self.writer.user_header().clone()));

        NippyJar::<SegmentHeader>::load(&previous_path)?.rename(new_path)?;

        // Opens the updated snapshot
        let (writer, data_path) = Self::open(segment, block, self.reader.clone())?;
        self.writer = writer;
        self.data_path = data_path;

        Ok(())
    }

    /// Appends a column to disk. It creates and loads a new snapshot file if block goes beyond the
    /// current block range.
    ///
    /// ATTENTION: It **requires** `self.commit` to be called manually
    fn append<T, F1>(
        &mut self,
        block: BlockNumber,
        segment: SnapshotSegment,
        column: T,
        initialize_segment: F1,
    ) -> ProviderResult<()>
    where
        T: Compact,
        F1: Fn(&mut SegmentHeader),
    {
        // We have finished the previous snapshot and must freeze it
        if block > self.writer.user_header().block_end() {
            // Commits offsets and new user_header to disk
            self.writer.commit()?;

            // We need to commit the changes done to `SegmentHeader` as well
            let previous_path = self.data_path.clone();
            let new_path = self
                .reader
                .directory()
                .join(segment.filename_from_header(self.writer.user_header().clone()));

            // Opens the new snapshot
            let (writer, data_path) = Self::open(segment, block, self.reader.clone())?;
            self.writer = writer;
            self.data_path = data_path;

            // Initializes the new segment header
            initialize_segment(self.writer.user_header_mut());

            // Updates the name of the previous snapshot
            NippyJar::<SegmentHeader>::load(&previous_path)?.rename(new_path)?;
        } else {
            self.writer.user_header_mut().increment()
        }

        self.append_column(column)?;

        Ok(())
    }

    /// Truncates a number of rows from disk. It delets and loads an older snapshot file if block
    /// goes beyond the start of the current block range.
    ///
    /// ATTENTION: It commits in the end.
    fn truncate(&mut self, segment: SnapshotSegment, mut num: u64) -> ProviderResult<()> {
        while num > 0 {
            let len = match segment {
                SnapshotSegment::Headers => self.writer.user_header().block_len(),
                SnapshotSegment::Transactions | SnapshotSegment::Receipts => {
                    self.writer.user_header().tx_len()
                }
            };

            if num >= len {
                // If there's more rows to delete than this snapshot contains, then just
                // delete the whole file and go to the next snapshot
                let previous_snap = self.data_path.clone();

                if self.writer.user_header().block_start() != 0 {
                    let (writer, data_path) = Self::open(
                        segment,
                        self.writer.user_header().block_start() - 1,
                        self.reader.clone(),
                    )?;
                    self.writer = writer;
                    self.data_path = data_path;
                }

                NippyJar::<SegmentHeader>::load(&previous_snap)?.delete()?;
            } else {
                let to_delete = len - num;
                self.writer.user_header_mut().prune(to_delete);
                self.writer.prune_rows(to_delete as usize)?;
            }
            num -= len;
        }

        self.commit()
    }

    /// Appends column to snapshot file.
    fn append_column<T: Compact>(&mut self, column: T) -> ProviderResult<()> {
        self.buf.clear();
        column.to_compact(&mut self.buf);

        self.writer.append_column(Some(Ok(&self.buf)))?;
        Ok(())
    }

    /// Appends to tx number-based snapshot file.
    fn append_with_tx_number<V: Compact>(
        &mut self,
        segment: SnapshotSegment,
        block: BlockNumber,
        tx_num: TxNumber,
        value: V,
    ) -> ProviderResult<()> {
        debug_assert!(self.writer.user_header().segment() == segment);

        if self.writer.user_header().tx_range().contains(&tx_num) {
            return Ok(())
        }

        self.append(block, segment, value, |segment_header| {
            let block_range = segment_header.block_start()..=segment_header.block_end();
            *segment_header = SegmentHeader::new(block_range, tx_num..=tx_num, segment);
        })
    }

    /// Appends transaction to snapshot file.
    pub fn append_transaction(
        &mut self,
        block: BlockNumber,
        tx_num: TxNumber,
        tx: TransactionSignedNoHash,
    ) -> ProviderResult<()> {
        self.append_with_tx_number(SnapshotSegment::Transactions, block, tx_num, tx)
    }

    /// Appends receipt to snapshot file.
    pub fn append_receipt(
        &mut self,
        block: BlockNumber,
        tx_num: TxNumber,
        receipt: Receipt,
    ) -> ProviderResult<()> {
        self.append_with_tx_number(SnapshotSegment::Receipts, block, tx_num, receipt)
    }

    /// Prunes `to_delete` number of transactions from snapshots.
    pub fn prune_transactions(&mut self, to_delete: u64) -> ProviderResult<()> {
        let segment = SnapshotSegment::Transactions;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, to_delete)
    }
}

impl<'a> Drop for SnapshotProviderRW<'a> {
    fn drop(&mut self) {
        let _ = self.commit();
    }
}

impl<'a> Deref for SnapshotProviderRW<'a> {
    type Target = SnapshotProvider;
    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

/// Each snapshot has a fixed number of blocks. This gives out the range where the requested block
/// is positioned.
fn find_range(interval: u64, block: u64) -> RangeInclusive<u64> {
    let start = (block / interval) * interval;
    let end = if block % interval == 0 { block } else { start + interval - 1 };
    start..=end
}
