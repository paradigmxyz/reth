use super::{find_fixed_range, SnapshotProvider, BLOCKS_PER_SNAPSHOT};
use reth_codecs::Compact;
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_nippy_jar::{NippyJar, NippyJarError, NippyJarWriter};
use reth_primitives::{
    snapshot::SegmentHeader, BlockNumber, Receipt, SnapshotSegment, TransactionSignedNoHash,
    TxNumber,
};
use std::{ops::Deref, path::PathBuf, sync::Arc};

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
        let block_range = find_fixed_range(BLOCKS_PER_SNAPSHOT, block);
        let (jar, path) = match reader.get_segment_provider_from_block(segment, block, None) {
            Ok(provider) => (NippyJar::load(provider.data_path())?, provider.data_path().into()),
            Err(ProviderError::MissingSnapshotBlock(_, _)) => {
                // TODO(joshie): if its a receipt segment, we can find out the actual range.
                let tx_range = reader.get_highest_snapshot_tx(segment).map(|tx| tx..=tx);
                let path = reader.directory().join(segment.filename(&block_range));
                (
                    NippyJar::new(
                        segment.columns(),
                        &path,
                        SegmentHeader::new(
                            *block_range.start()..=*block_range.start(),
                            tx_range,
                            segment,
                        ),
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

    /// Commits configuration changes to disk and updates the reader index with the new changes.
    pub fn commit(&mut self) -> ProviderResult<()> {
        // Commits offsets and new user_header to disk
        self.writer.commit()?;

        // TODO(joshie): update the index without re-iterating all snapshots
        self.reader.update_index()?;

        Ok(())
    }

    /// Appends a column to disk. It creates and loads a new snapshot file if block goes beyond the
    /// current block range.
    ///
    /// ATTENTION: It **requires** `self.commit` to be called manually
    fn append<T, F1, F2>(
        &mut self,
        block: BlockNumber,
        segment: SnapshotSegment,
        column: T,
        initialize_segment: F1,
        update_segment: F2,
    ) -> ProviderResult<()>
    where
        T: Compact,
        F1: Fn(&mut SegmentHeader),
        F2: Fn(&mut SegmentHeader),
    {
        // We have finished the previous snapshot and must freeze it
        if block >
            *find_fixed_range(BLOCKS_PER_SNAPSHOT, self.writer.user_header().block_end()).end()
        {
            // Commits offsets and new user_header to disk
            self.writer.commit()?;

            dbg!(&self.writer.user_header());

            // Opens the new snapshot
            let (writer, data_path) = Self::open(segment, block, self.reader.clone())?;
            self.writer = writer;
            self.data_path = data_path;

            // Initializes the new segment header
            initialize_segment(self.writer.user_header_mut());
        } else {
            update_segment(self.writer.user_header_mut());
        }

        self.append_column(column)?;

        Ok(())
    }

    /// Truncates a number of rows from disk. It delets and loads an older snapshot file if block
    /// goes beyond the start of the current block range.
    ///
    /// **last_block** should be passed only with transaction based segments.
    ///
    /// ATTENTION: It **requires** `self.commit` to be called manually
    fn truncate(
        &mut self,
        segment: SnapshotSegment,
        mut num_rows: u64,
        last_block: Option<u64>,
    ) -> ProviderResult<()> {
        while num_rows > 0 {
            let len = match segment {
                SnapshotSegment::Headers => self.writer.user_header().block_len(),
                SnapshotSegment::Transactions | SnapshotSegment::Receipts => {
                    self.writer.user_header().tx_len()
                }
            };

            if num_rows >= len {
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

                num_rows -= len;
            } else {
                // Update `SegmentHeader`
                self.writer.user_header_mut().prune(num_rows);

                // Only Transactions and Receipts
                if let Some(last_block) = last_block {
                    let header = self.writer.user_header_mut();
                    header.set_block_range(header.block_start()..=last_block);
                }

                // Truncate data
                self.writer.prune_rows(num_rows as usize)?;
                num_rows = 0;
            }
        }

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
    fn append_with_tx_number<V: Compact>(
        &mut self,
        segment: SnapshotSegment,
        block: BlockNumber,
        tx_num: TxNumber,
        value: V,
    ) -> ProviderResult<()> {
        debug_assert!(self.writer.user_header().segment() == segment);

        if self.writer.user_header().tx_range().is_some_and(|range| range.contains(&tx_num)) {
            return Ok(())
        }

        self.append(
            block,
            segment,
            value,
            |segment_header| {
                let block_start = *find_fixed_range(BLOCKS_PER_SNAPSHOT, block).start();
                *segment_header =
                    SegmentHeader::new(block_start..=block_start, Some(tx_num..=tx_num), segment);
            },
            |segment_header| {
                if block > segment_header.block_end() {
                    segment_header.increment_block()
                }
                segment_header.increment_tx()
            },
        )
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
    pub fn prune_transactions(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let segment = SnapshotSegment::Transactions;
        debug_assert!(self.writer.user_header().segment() == segment);

        self.truncate(segment, to_delete, Some(last_block))
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
