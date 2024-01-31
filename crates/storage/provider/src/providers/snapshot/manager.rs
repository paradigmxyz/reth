use super::{
    LoadedJar, SnapshotJarProvider, SnapshotProviderRW, SnapshotProviderRWRefMut,
    BLOCKS_PER_SNAPSHOT,
};
use crate::{
    to_range, BlockHashReader, BlockNumReader, BlockReader, BlockSource, HeaderProvider,
    ReceiptProvider, StatsReader, TransactionVariant, TransactionsProvider,
    TransactionsProviderExt, WithdrawalsProvider,
};
use dashmap::{mapref::entry::Entry as DashMapEntry, DashMap};
use parking_lot::RwLock;
use reth_db::{
    codecs::CompactU256,
    models::StoredBlockBodyIndices,
    snapshot::{
        iter_snapshots, ColumnSelectorOne, HeaderMask, ReceiptMask, SnapshotCursor, TransactionMask,
    },
    table::Table,
    tables, RawValue,
};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_nippy_jar::NippyJar;
use reth_primitives::{
    snapshot::{find_fixed_range, HighestSnapshots, SegmentHeader},
    Address, Block, BlockHash, BlockHashOrNumber, BlockNumber, BlockWithSenders, ChainInfo, Header,
    Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader, SnapshotSegment, TransactionMeta,
    TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, Withdrawal, B256, U256,
};
use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap},
    ops::{Range, RangeBounds, RangeInclusive},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Alias type for a map that can be queried for block ranges from a transaction
/// segment respectively. It uses `TxNumber` to represent the transaction end of a snapshot range.
type SegmentRanges = HashMap<SnapshotSegment, BTreeMap<TxNumber, RangeInclusive<BlockNumber>>>;

/// [`SnapshotProvider`] manages all existing [`SnapshotJarProvider`].
#[derive(Debug, Default)]
pub struct SnapshotProvider {
    /// Maintains a map which allows for concurrent access to different `NippyJars`, over different
    /// segments and ranges.
    map: DashMap<(BlockNumber, SnapshotSegment), LoadedJar>,
    /// Max snapshotted block for each snapshot segment
    snapshots_max_block: RwLock<HashMap<SnapshotSegment, u64>>,
    /// Available snapshot block ranges on disk indexed by max transactions.
    snapshots_tx_index: RwLock<SegmentRanges>,
    /// Directory where snapshots are located
    path: PathBuf,
    /// Whether [`SnapshotJarProvider`] loads filters into memory. If not, `by_hash` queries won't
    /// be able to be queried directly.
    load_filters: bool,
    /// Maintains a map of Snapshot writers for each [`SnapshotSegment`]
    writers: DashMap<SnapshotSegment, SnapshotProviderRW<'static>>,
}

impl SnapshotProvider {
    /// Creates a new [`SnapshotProvider`].
    pub fn new(path: impl AsRef<Path>) -> ProviderResult<Self> {
        let provider = Self {
            map: Default::default(),
            writers: Default::default(),
            snapshots_max_block: Default::default(),
            snapshots_tx_index: Default::default(),
            path: path.as_ref().to_path_buf(),
            load_filters: false,
        };

        provider.initialize_index()?;
        Ok(provider)
    }

    /// Loads filters into memory when creating a [`SnapshotJarProvider`].
    pub fn with_filters(mut self) -> Self {
        self.load_filters = true;
        self
    }

    /// Gets the [`SnapshotJarProvider`] of the requested segment and block.
    pub fn get_segment_provider_from_block(
        &self,
        segment: SnapshotSegment,
        block: BlockNumber,
        path: Option<&Path>,
    ) -> ProviderResult<SnapshotJarProvider<'_>> {
        self.get_segment_provider(
            segment,
            || self.get_segment_ranges_from_block(segment, block),
            path,
        )?
        .ok_or_else(|| ProviderError::MissingSnapshotBlock(segment, block))
    }

    /// Gets the [`SnapshotJarProvider`] of the requested segment and transaction.
    pub fn get_segment_provider_from_transaction(
        &self,
        segment: SnapshotSegment,
        tx: TxNumber,
        path: Option<&Path>,
    ) -> ProviderResult<SnapshotJarProvider<'_>> {
        self.get_segment_provider(
            segment,
            || self.get_segment_ranges_from_transaction(segment, tx),
            path,
        )?
        .ok_or_else(|| ProviderError::MissingSnapshotTx(segment, tx))
    }

    /// Gets the [`SnapshotJarProvider`] of the requested segment and block or transaction.
    ///
    /// `fn_range` should make sure the range goes through `find_fixed_range`.
    pub fn get_segment_provider(
        &self,
        segment: SnapshotSegment,
        fn_range: impl Fn() -> Option<RangeInclusive<BlockNumber>>,
        path: Option<&Path>,
    ) -> ProviderResult<Option<SnapshotJarProvider<'_>>> {
        // If we have a path, then get the block range from its name.
        // Otherwise, check `self.available_snapshots`
        let block_range = match path {
            Some(path) => SnapshotSegment::parse_filename(
                &path
                    .file_name()
                    .ok_or_else(|| ProviderError::MissingSnapshotPath(segment, path.to_path_buf()))?
                    .to_string_lossy(),
            )
            .and_then(|(parsed_segment, block_range)| {
                if parsed_segment == segment {
                    return Some(block_range)
                }
                None
            }),
            None => fn_range(),
        };

        // Return cached `LoadedJar` or insert it for the first time, and then, return it.
        if let Some(block_range) = block_range {
            return Ok(Some(self.get_or_create_jar_provider(segment, &block_range)?))
        }

        Ok(None)
    }

    /// Given a segment and block range it removes the cached provider from the map.
    pub fn remove_cached_provider(
        &self,
        segment: SnapshotSegment,
        fixed_block_range_end: BlockNumber,
    ) {
        self.map.remove(&(fixed_block_range_end, segment));
    }

    /// Given a segment and block range it returns a cached
    /// [`SnapshotJarProvider`]. TODO(joshie): we should check the size and pop N if there's too
    /// many.
    fn get_or_create_jar_provider(
        &self,
        segment: SnapshotSegment,
        fixed_block_range: &RangeInclusive<u64>,
    ) -> ProviderResult<SnapshotJarProvider<'_>> {
        let key = (*fixed_block_range.end(), segment);
        if let Some(jar) = self.map.get(&key) {
            Ok(jar.into())
        } else {
            let jar = NippyJar::load(&self.path.join(segment.filename(fixed_block_range))).map(
                |jar| {
                    if self.load_filters {
                        return jar.load_filters()
                    }
                    Ok(jar)
                },
            )??;

            self.map.insert(key, LoadedJar::new(jar)?);
            Ok(self.map.get(&key).expect("qed").into())
        }
    }

    /// Gets a snapshot segment's block range from the provider inner block
    /// index.
    fn get_segment_ranges_from_block(
        &self,
        segment: SnapshotSegment,
        block: u64,
    ) -> Option<RangeInclusive<BlockNumber>> {
        self.snapshots_max_block
            .read()
            .get(&segment)
            .filter(|max| **max >= block)
            .map(|_| find_fixed_range(block))
    }

    /// Gets a snapshot segment's fixed block range from the provider inner
    /// transaction index.
    fn get_segment_ranges_from_transaction(
        &self,
        segment: SnapshotSegment,
        tx: u64,
    ) -> Option<RangeInclusive<BlockNumber>> {
        let snapshots = self.snapshots_tx_index.read();
        let segment_snapshots = snapshots.get(&segment)?;

        // It's more probable that the request comes from a newer tx height, so we iterate
        // the snapshots in reverse.
        let mut snapshots_rev_iter = segment_snapshots.iter().rev().peekable();

        while let Some((tx_end, block_range)) = snapshots_rev_iter.next() {
            if tx > *tx_end {
                // request tx is higher than highest snapshot tx
                return None
            }
            let tx_start = snapshots_rev_iter.peek().map(|(tx_end, _)| *tx_end + 1).unwrap_or(0);
            if tx_start <= tx {
                return Some(find_fixed_range(*block_range.end()))
            }
        }
        None
    }

    /// Updates the inner transaction and block indexes alongside the internal cached providers in
    /// `self.map`.
    ///
    /// Any entry higher than `segment_max_block` will be deleted from the previous structures.
    ///
    /// If `segment_max_block` is None it means there's no static file for this segment.
    pub fn update_index(
        &self,
        segment: SnapshotSegment,
        segment_max_block: Option<BlockNumber>,
    ) -> ProviderResult<()> {
        let mut max_block = self.snapshots_max_block.write();
        let mut tx_index = self.snapshots_tx_index.write();

        match segment_max_block {
            Some(segment_max_block) => {
                // Update the max block for the segment
                max_block.insert(segment, segment_max_block);
                let fixed_range = find_fixed_range(segment_max_block);

                let jar = NippyJar::<SegmentHeader>::load(
                    &self.path.join(segment.filename(&fixed_range)),
                )?;

                // Updates the tx index by first removing all entries which have a higher
                // block_start than our current static file.
                if let Some(tx_range) = jar.user_header().tx_range() {
                    let tx_end = *tx_range.end();

                    // Current block range has the same block start as `fixed_range``, but block end
                    // might be different if we are still filling this static file.
                    let current_block_range = jar.user_header().block_range();

                    // Considering that `update_index` is called when we either append/truncate, we
                    // are sure that we are handling the latest data points.
                    //
                    // Here we remove every entry of the index that has a block start higher or
                    // equal than our current one. This is important in the case
                    // that we prune a lot of rows resulting in a file (and thus
                    // a higher block range) deletion.
                    tx_index
                        .entry(segment)
                        .and_modify(|index| {
                            index
                                .retain(|_, block_range| block_range.start() < fixed_range.start());
                            index.insert(tx_end, current_block_range.clone());
                        })
                        .or_insert_with(|| BTreeMap::from([(tx_end, current_block_range)]));
                } else if let Some(1) = tx_index.get(&segment).map(|index| index.len()) {
                    // Only happens if we unwind all the txs/receipts from the first static file.
                    // Should only happen in test scenarios.
                    if matches!(segment, SnapshotSegment::Receipts | SnapshotSegment::Transactions)
                    {
                        tx_index.remove(&segment);
                    }
                }

                // Update the cached provider.
                self.map.insert((*fixed_range.end(), segment), LoadedJar::new(jar)?);

                // Delete any cached provider that no longer has an associated jar.
                self.map.retain(|(end, seg), _| !(*seg == segment && *end > *fixed_range.end()));
            }
            None => {
                tx_index.remove(&segment);
                max_block.remove(&segment);
            }
        };

        Ok(())
    }

    /// Initializes the inner transaction and block index
    pub fn initialize_index(&self) -> ProviderResult<()> {
        let mut max_block = self.snapshots_max_block.write();
        let mut tx_index = self.snapshots_tx_index.write();

        tx_index.clear();

        for (segment, ranges) in iter_snapshots(&self.path)? {
            // Update last block for each segment
            if let Some((block_range, _)) = ranges.last() {
                max_block.insert(segment, *block_range.end());
            }

            // Update tx -> block_range index
            for (block_range, tx_range) in ranges {
                if let Some(tx_range) = tx_range {
                    let tx_end = *tx_range.end();

                    match tx_index.entry(segment) {
                        Entry::Occupied(mut index) => {
                            index.get_mut().insert(tx_end, block_range);
                        }
                        Entry::Vacant(index) => {
                            index.insert(BTreeMap::from([(tx_end, block_range)]));
                        }
                    };
                }
            }
        }

        Ok(())
    }

    /// Gets the highest snapshot block if it exists for a snapshot segment.
    pub fn get_highest_snapshot_block(&self, segment: SnapshotSegment) -> Option<BlockNumber> {
        self.snapshots_max_block.read().get(&segment).copied()
    }

    /// Gets the highest snapshotted transaction.
    pub fn get_highest_snapshot_tx(&self, segment: SnapshotSegment) -> Option<TxNumber> {
        self.snapshots_tx_index
            .read()
            .get(&segment)
            .and_then(|index| index.last_key_value().map(|(last_tx, _)| *last_tx))
    }

    /// Gets the highest snapshotted blocks for all segments.
    pub fn get_highest_snapshots(&self) -> HighestSnapshots {
        HighestSnapshots {
            headers: self.get_highest_snapshot_block(SnapshotSegment::Headers),
            receipts: self.get_highest_snapshot_block(SnapshotSegment::Receipts),
            transactions: self.get_highest_snapshot_block(SnapshotSegment::Transactions),
        }
    }

    /// Iterates through segment snapshots in reverse order, executing a function until it returns
    /// some object. Useful for finding objects by [`TxHash`] or [`BlockHash`].
    pub fn find_snapshot<T>(
        &self,
        segment: SnapshotSegment,
        func: impl Fn(SnapshotJarProvider<'_>) -> ProviderResult<Option<T>>,
    ) -> ProviderResult<Option<T>> {
        if let Some(highest_block) = self.get_highest_snapshot_block(segment) {
            let mut range = find_fixed_range(highest_block);
            while *range.end() > 0 {
                if let Some(res) = func(self.get_or_create_jar_provider(segment, &range)?)? {
                    return Ok(Some(res))
                }
                range = range.start().saturating_sub(BLOCKS_PER_SNAPSHOT)..=
                    range.end().saturating_sub(BLOCKS_PER_SNAPSHOT);
            }
        }

        Ok(None)
    }

    /// Fetches data within a specified range across multiple snapshot files.
    ///
    /// This function iteratively retrieves data using `get_fn` for each item in the given range.
    /// It continues fetching until the end of the range is reached or the provided `predicate`
    /// returns false.
    pub fn fetch_range_with_predicate<T, F, P>(
        &self,
        segment: SnapshotSegment,
        range: Range<u64>,
        get_fn: F,
        mut predicate: P,
    ) -> ProviderResult<Vec<T>>
    where
        F: Fn(&mut SnapshotCursor<'_>, u64) -> ProviderResult<Option<T>>,
        P: FnMut(&T) -> bool,
    {
        let get_provider = |start: u64| match segment {
            SnapshotSegment::Headers => self.get_segment_provider_from_block(segment, start, None),
            SnapshotSegment::Transactions | SnapshotSegment::Receipts => {
                self.get_segment_provider_from_transaction(segment, start, None)
            }
        };

        let mut result = Vec::with_capacity((range.end - range.start).min(100) as usize);
        let mut provider = get_provider(range.start)?;
        let mut cursor = provider.cursor()?;

        // advances number in range
        'outer: for number in range {
            // advances snapshot files if `get_fn` returns None
            'inner: loop {
                match get_fn(&mut cursor, number)? {
                    Some(res) => {
                        if !predicate(&res) {
                            break 'outer
                        }
                        result.push(res);
                        break 'inner
                    }
                    None => {
                        provider = get_provider(number)?;
                        cursor = provider.cursor()?;
                    }
                }
            }
        }

        Ok(result)
    }

    /// Fetches data within a specified range across multiple snapshot files.
    ///
    /// Returns an iterator over the data
    pub fn fetch_range_iter<'a, T, F>(
        &'a self,
        segment: SnapshotSegment,
        range: Range<u64>,
        get_fn: F,
    ) -> ProviderResult<impl Iterator<Item = ProviderResult<T>> + 'a>
    where
        F: Fn(&mut SnapshotCursor<'_>, u64) -> ProviderResult<Option<T>> + 'a,
        T: std::fmt::Debug,
    {
        let get_provider = move |start: u64| match segment {
            SnapshotSegment::Headers => self.get_segment_provider_from_block(segment, start, None),
            SnapshotSegment::Transactions | SnapshotSegment::Receipts => {
                self.get_segment_provider_from_transaction(segment, start, None)
            }
        };
        let mut provider = get_provider(range.start)?;

        Ok(range.filter_map(move |number| {
            match get_fn(&mut provider.cursor().ok()?, number).transpose() {
                Some(result) => Some(result),
                None => {
                    provider = get_provider(number).ok()?;
                    get_fn(&mut provider.cursor().ok()?, number).transpose()
                }
            }
        }))
    }

    /// Returns directory where snapshots are located.
    pub fn directory(&self) -> &Path {
        &self.path
    }

    /// Retrieves data from the database or snapshot, wherever it's available.
    ///
    /// # Arguments
    /// * `segment` - The segment of the snapshot to check against.
    /// * `index_key` - Requested index key, usually a block or transaction number.
    /// * `fetch_from_snapshot` - A closure that defines how to fetch the data from the snapshot
    ///   provider.
    /// * `fetch_from_database` - A closure that defines how to fetch the data from the database
    ///   when the snapshot doesn't contain the required data or is not available.
    pub fn get_with_snapshot_or_database<T, FS, FD>(
        &self,
        segment: SnapshotSegment,
        number: u64,
        fetch_from_snapshot: FS,
        fetch_from_database: FD,
    ) -> ProviderResult<Option<T>>
    where
        FS: Fn(&SnapshotProvider) -> ProviderResult<Option<T>>,
        FD: Fn() -> ProviderResult<Option<T>>,
    {
        // If there is, check the maximum block or transaction number of the segment.
        let snapshot_upper_bound = match segment {
            SnapshotSegment::Headers => self.get_highest_snapshot_block(segment),
            SnapshotSegment::Transactions | SnapshotSegment::Receipts => {
                self.get_highest_snapshot_tx(segment)
            }
        };

        if snapshot_upper_bound.map_or(false, |snapshot_upper_bound| snapshot_upper_bound >= number)
        {
            return fetch_from_snapshot(self)
        }
        fetch_from_database()
    }

    /// Gets data within a specified range, potentially spanning different snapshots and database.
    ///
    /// # Arguments
    /// * `segment` - The segment of the snapshot to query.
    /// * `block_range` - The range of data to fetch.
    /// * `fetch_from_snapshot` - A function to fetch data from the snapshot.
    /// * `fetch_from_database` - A function to fetch data from the database.
    /// * `predicate` - A function used to evaluate each item in the fetched data. Fetching is
    ///   terminated when this function returns false, thereby filtering the data based on the
    ///   provided condition.
    pub fn get_range_with_snapshot_or_database<T, P, FS, FD>(
        &self,
        segment: SnapshotSegment,
        mut block_or_tx_range: Range<u64>,
        fetch_from_snapshot: FS,
        mut fetch_from_database: FD,
        mut predicate: P,
    ) -> ProviderResult<Vec<T>>
    where
        FS: Fn(&SnapshotProvider, Range<u64>, &mut P) -> ProviderResult<Vec<T>>,
        FD: FnMut(Range<u64>, P) -> ProviderResult<Vec<T>>,
        P: FnMut(&T) -> bool,
    {
        let mut data = Vec::new();

        // If there is, check the maximum block or transaction number of the segment.
        if let Some(snapshot_upper_bound) = match segment {
            SnapshotSegment::Headers => self.get_highest_snapshot_block(segment),
            SnapshotSegment::Transactions | SnapshotSegment::Receipts => {
                self.get_highest_snapshot_tx(segment)
            }
        } {
            if block_or_tx_range.start <= snapshot_upper_bound {
                let end = block_or_tx_range.end.min(snapshot_upper_bound + 1);
                data.extend(fetch_from_snapshot(
                    self,
                    block_or_tx_range.start..end,
                    &mut predicate,
                )?);
                block_or_tx_range.start = end;
            }
        }

        if block_or_tx_range.end > block_or_tx_range.start {
            data.extend(fetch_from_database(block_or_tx_range, predicate)?)
        }

        Ok(data)
    }
}

/// Helper trait to manage different [`SnapshotProviderRW`] of an `Arc<SnapshotProvider`
pub trait SnapshotWriter {
    /// Returns a mutable reference to a [`SnapshotProviderRW`] of a [`SnapshotSegment`].
    fn get_writer(
        &self,
        block: BlockNumber,
        segment: SnapshotSegment,
    ) -> ProviderResult<SnapshotProviderRWRefMut<'_>>;

    /// Returns a mutable reference to a [`SnapshotProviderRW`] of the latest [`SnapshotSegment`].
    fn latest_writer(
        &self,
        segment: SnapshotSegment,
    ) -> ProviderResult<SnapshotProviderRWRefMut<'_>>;

    /// Commits all changes of all [`SnapshotProviderRW`] of all [`SnapshotSegment`].
    fn commit(&self) -> ProviderResult<()>;
}

impl SnapshotWriter for Arc<SnapshotProvider> {
    fn get_writer(
        &self,
        block: BlockNumber,
        segment: SnapshotSegment,
    ) -> ProviderResult<SnapshotProviderRWRefMut<'_>> {
        tracing::trace!(target: "providers::static_file", ?block, ?segment, "Getting static file writer.");
        Ok(match self.writers.entry(segment) {
            DashMapEntry::Occupied(entry) => entry.into_ref(),
            DashMapEntry::Vacant(entry) => {
                entry.insert(SnapshotProviderRW::new(segment, block, self.clone())?)
            }
        })
    }

    fn latest_writer(
        &self,
        segment: SnapshotSegment,
    ) -> ProviderResult<SnapshotProviderRWRefMut<'_>> {
        self.get_writer(self.get_highest_snapshot_block(segment).unwrap_or_default(), segment)
    }

    fn commit(&self) -> ProviderResult<()> {
        for mut writer in self.writers.iter_mut() {
            writer.commit()?;
        }
        Ok(())
    }
}

impl HeaderProvider for SnapshotProvider {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.find_snapshot(SnapshotSegment::Headers, |jar_provider| {
            Ok(jar_provider
                .cursor()?
                .get_two::<HeaderMask<Header, BlockHash>>(block_hash.into())?
                .and_then(|(header, hash)| {
                    if &hash == block_hash {
                        return Some(header)
                    }
                    None
                }))
        })
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?
            .header_by_number(num)
    }

    fn header_td(&self, block_hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.find_snapshot(SnapshotSegment::Headers, |jar_provider| {
            Ok(jar_provider
                .cursor()?
                .get_two::<HeaderMask<CompactU256, BlockHash>>(block_hash.into())?
                .and_then(|(td, hash)| (&hash == block_hash).then_some(td.0)))
        })
    }

    fn header_td_by_number(&self, num: BlockNumber) -> ProviderResult<Option<U256>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?
            .header_td_by_number(num)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.fetch_range_with_predicate(
            SnapshotSegment::Headers,
            to_range(range),
            |cursor, number| cursor.get_one::<HeaderMask<Header>>(number.into()),
            |_| true,
        )
    }

    fn sealed_header(&self, num: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?
            .sealed_header(num)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.fetch_range_with_predicate(
            SnapshotSegment::Headers,
            to_range(range),
            |cursor, number| {
                Ok(cursor
                    .get_two::<HeaderMask<Header, BlockHash>>(number.into())?
                    .map(|(header, hash)| header.seal(hash)))
            },
            predicate,
        )
    }
}

impl BlockHashReader for SnapshotProvider {
    fn block_hash(&self, num: u64) -> ProviderResult<Option<B256>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?.block_hash(num)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.fetch_range_with_predicate(
            SnapshotSegment::Headers,
            start..end,
            |cursor, number| cursor.get_one::<HeaderMask<BlockHash>>(number.into()),
            |_| true,
        )
    }
}

impl ReceiptProvider for SnapshotProvider {
    fn receipt(&self, num: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.get_segment_provider_from_transaction(SnapshotSegment::Receipts, num, None)?
            .receipt(num)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        if let Some(num) = self.transaction_id(hash)? {
            return self.receipt(num)
        }
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        unreachable!()
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        self.fetch_range_with_predicate(
            SnapshotSegment::Receipts,
            to_range(range),
            |cursor, number| cursor.get_one::<ReceiptMask<Receipt>>(number.into()),
            |_| true,
        )
    }
}

impl TransactionsProviderExt for SnapshotProvider {
    fn transaction_hashes_by_range(
        &self,
        tx_range: Range<TxNumber>,
    ) -> ProviderResult<Vec<(TxHash, TxNumber)>> {
        self.fetch_range_with_predicate(
            SnapshotSegment::Transactions,
            tx_range,
            |cursor, number| {
                let tx =
                    cursor.get_one::<TransactionMask<TransactionSignedNoHash>>(number.into())?;
                Ok(tx.map(|tx| (tx.hash(), cursor.number())))
            },
            |_| true,
        )
    }
}

impl TransactionsProvider for SnapshotProvider {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.find_snapshot(SnapshotSegment::Transactions, |jar_provider| {
            let mut cursor = jar_provider.cursor()?;
            if cursor
                .get_one::<TransactionMask<TransactionSignedNoHash>>((&tx_hash).into())?
                .and_then(|tx| (tx.hash() == tx_hash).then_some(tx))
                .is_some()
            {
                Ok(Some(cursor.number()))
            } else {
                Ok(None)
            }
        })
    }

    fn transaction_by_id(&self, num: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.get_segment_provider_from_transaction(SnapshotSegment::Transactions, num, None)?
            .transaction_by_id(num)
    }

    fn transaction_by_id_no_hash(
        &self,
        num: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.get_segment_provider_from_transaction(SnapshotSegment::Transactions, num, None)?
            .transaction_by_id_no_hash(num)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        self.find_snapshot(SnapshotSegment::Transactions, |jar_provider| {
            Ok(jar_provider
                .cursor()?
                .get_one::<TransactionMask<TransactionSignedNoHash>>((&hash).into())?
                .map(|tx| tx.with_hash())
                .and_then(|tx| (tx.hash_ref() == &hash).then_some(tx)))
        })
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        let txes = self.transactions_by_tx_range(range)?;
        TransactionSignedNoHash::recover_signers(&txes, txes.len())
            .ok_or(ProviderError::SenderRecoveryError)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.fetch_range_with_predicate(
            SnapshotSegment::Transactions,
            to_range(range),
            |cursor, number| {
                cursor.get_one::<TransactionMask<TransactionSignedNoHash>>(number.into())
            },
            |_| true,
        )
    }

    fn raw_transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<RawValue<TransactionSignedNoHash>>> {
        self.fetch_range_with_predicate(
            SnapshotSegment::Transactions,
            to_range(range),
            |cursor, number| {
                cursor.get(number.into(), <TransactionMask<TransactionSignedNoHash>>::MASK).map(
                    |result| {
                        result.map(|row| {
                            RawValue::<TransactionSignedNoHash>::from_vec(row[0].to_vec())
                        })
                    },
                )
            },
            |_| true,
        )
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(self.transaction_by_id_no_hash(id)?.and_then(|tx| tx.recover_signer()))
    }
}

/* Cannot be successfully implemented but must exist for trait requirements */

impl BlockNumReader for SnapshotProvider {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }
}

impl BlockReader for SnapshotProvider {
    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Block>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn block(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }
}

impl WithdrawalsProvider for SnapshotProvider {
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> ProviderResult<Option<Vec<Withdrawal>>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        // Required data not present in snapshots
        Err(ProviderError::UnsupportedProvider)
    }
}

impl StatsReader for SnapshotProvider {
    fn count_entries<T: Table>(&self) -> ProviderResult<usize> {
        match T::NAME {
            tables::CanonicalHeaders::NAME | tables::Headers::NAME | tables::HeaderTD::NAME => {
                Ok(self
                    .get_highest_snapshot_block(SnapshotSegment::Headers)
                    .map(|block| block + 1)
                    .unwrap_or_default() as usize)
            }
            tables::Receipts::NAME => Ok(self
                .get_highest_snapshot_tx(SnapshotSegment::Receipts)
                .map(|receipts| receipts + 1)
                .unwrap_or_default() as usize),
            tables::Transactions::NAME => Ok(self
                .get_highest_snapshot_tx(SnapshotSegment::Transactions)
                .map(|txs| txs + 1)
                .unwrap_or_default() as usize),
            _ => Err(ProviderError::UnsupportedProvider),
        }
    }
}
