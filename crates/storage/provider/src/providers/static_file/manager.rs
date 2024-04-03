use super::{
    metrics::StaticFileProviderMetrics, LoadedJar, StaticFileJarProvider, StaticFileProviderRW,
    StaticFileProviderRWRefMut, BLOCKS_PER_STATIC_FILE,
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
    static_file::{iter_static_files, HeaderMask, ReceiptMask, StaticFileCursor, TransactionMask},
    table::Table,
    tables,
};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_nippy_jar::NippyJar;
use reth_primitives::{
    keccak256,
    static_file::{find_fixed_range, HighestStaticFiles, SegmentHeader, SegmentRangeInclusive},
    Address, Block, BlockHash, BlockHashOrNumber, BlockNumber, BlockWithSenders, ChainInfo, Header,
    Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader, StaticFileSegment, TransactionMeta,
    TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, Withdrawal, Withdrawals, B256,
    U256,
};
use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap},
    ops::{Deref, Range, RangeBounds, RangeInclusive},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
};
use tracing::warn;

/// Alias type for a map that can be queried for block ranges from a transaction
/// segment respectively. It uses `TxNumber` to represent the transaction end of a static file
/// range.
type SegmentRanges = HashMap<StaticFileSegment, BTreeMap<TxNumber, SegmentRangeInclusive>>;

/// [`StaticFileProvider`] manages all existing [`StaticFileJarProvider`].
#[derive(Debug, Default, Clone)]
pub struct StaticFileProvider(pub(crate) Arc<StaticFileProviderInner>);

impl StaticFileProvider {
    /// Creates a new [`StaticFileProvider`].
    pub fn new(path: impl AsRef<Path>) -> ProviderResult<Self> {
        let provider = Self(Arc::new(StaticFileProviderInner::new(path)?));
        provider.initialize_index()?;
        Ok(provider)
    }
}

impl Deref for StaticFileProvider {
    type Target = StaticFileProviderInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// [`StaticFileProviderInner`] manages all existing [`StaticFileJarProvider`].
#[derive(Debug, Default)]
pub struct StaticFileProviderInner {
    /// Maintains a map which allows for concurrent access to different `NippyJars`, over different
    /// segments and ranges.
    map: DashMap<(BlockNumber, StaticFileSegment), LoadedJar>,
    /// Max static file block for each segment
    static_files_max_block: RwLock<HashMap<StaticFileSegment, u64>>,
    /// Available static file block ranges on disk indexed by max transactions.
    static_files_tx_index: RwLock<SegmentRanges>,
    /// Directory where static_files are located
    path: PathBuf,
    /// Whether [`StaticFileJarProvider`] loads filters into memory. If not, `by_hash` queries
    /// won't be able to be queried directly.
    load_filters: bool,
    /// Maintains a map of StaticFile writers for each [`StaticFileSegment`]
    writers: DashMap<StaticFileSegment, StaticFileProviderRW>,
    metrics: Option<Arc<StaticFileProviderMetrics>>,
}

impl StaticFileProviderInner {
    /// Creates a new [`StaticFileProviderInner`].
    fn new(path: impl AsRef<Path>) -> ProviderResult<Self> {
        let provider = Self {
            map: Default::default(),
            writers: Default::default(),
            static_files_max_block: Default::default(),
            static_files_tx_index: Default::default(),
            path: path.as_ref().to_path_buf(),
            load_filters: false,
            metrics: None,
        };

        Ok(provider)
    }
}

impl StaticFileProvider {
    /// Loads filters into memory when creating a [`StaticFileJarProvider`].
    pub fn with_filters(self) -> Self {
        let mut provider =
            Arc::try_unwrap(self.0).expect("should be called when initializing only");
        provider.load_filters = true;
        Self(Arc::new(provider))
    }

    /// Enables metrics on the [`StaticFileProvider`].
    pub fn with_metrics(self) -> Self {
        let mut provider =
            Arc::try_unwrap(self.0).expect("should be called when initializing only");
        provider.metrics = Some(Arc::new(StaticFileProviderMetrics::default()));
        Self(Arc::new(provider))
    }

    /// Reports metrics for the static files.
    pub fn report_metrics(&self) -> ProviderResult<()> {
        let Some(metrics) = &self.metrics else { return Ok(()) };

        let static_files =
            iter_static_files(&self.path).map_err(|e| ProviderError::NippyJar(e.to_string()))?;
        for (segment, ranges) in static_files {
            let mut entries = 0;
            let mut size = 0;

            for (block_range, _) in &ranges {
                let fixed_block_range = find_fixed_range(block_range.start());
                let jar_provider = self
                    .get_segment_provider(segment, || Some(fixed_block_range), None)?
                    .ok_or(ProviderError::MissingStaticFileBlock(segment, block_range.start()))?;

                entries += jar_provider.rows();

                let data_size = reth_primitives::fs::metadata(jar_provider.data_path())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let index_size = reth_primitives::fs::metadata(jar_provider.index_path())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let offsets_size = reth_primitives::fs::metadata(jar_provider.offsets_path())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let config_size = reth_primitives::fs::metadata(jar_provider.config_path())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();

                size += data_size + index_size + offsets_size + config_size;
            }

            metrics.record_segment(segment, size, ranges.len(), entries);
        }

        Ok(())
    }

    /// Gets the [`StaticFileJarProvider`] of the requested segment and block.
    pub fn get_segment_provider_from_block(
        &self,
        segment: StaticFileSegment,
        block: BlockNumber,
        path: Option<&Path>,
    ) -> ProviderResult<StaticFileJarProvider<'_>> {
        self.get_segment_provider(
            segment,
            || self.get_segment_ranges_from_block(segment, block),
            path,
        )?
        .ok_or_else(|| ProviderError::MissingStaticFileBlock(segment, block))
    }

    /// Gets the [`StaticFileJarProvider`] of the requested segment and transaction.
    pub fn get_segment_provider_from_transaction(
        &self,
        segment: StaticFileSegment,
        tx: TxNumber,
        path: Option<&Path>,
    ) -> ProviderResult<StaticFileJarProvider<'_>> {
        self.get_segment_provider(
            segment,
            || self.get_segment_ranges_from_transaction(segment, tx),
            path,
        )?
        .ok_or_else(|| ProviderError::MissingStaticFileTx(segment, tx))
    }

    /// Gets the [`StaticFileJarProvider`] of the requested segment and block or transaction.
    ///
    /// `fn_range` should make sure the range goes through `find_fixed_range`.
    pub fn get_segment_provider(
        &self,
        segment: StaticFileSegment,
        fn_range: impl Fn() -> Option<SegmentRangeInclusive>,
        path: Option<&Path>,
    ) -> ProviderResult<Option<StaticFileJarProvider<'_>>> {
        // If we have a path, then get the block range from its name.
        // Otherwise, check `self.available_static_files`
        let block_range = match path {
            Some(path) => StaticFileSegment::parse_filename(
                &path
                    .file_name()
                    .ok_or_else(|| {
                        ProviderError::MissingStaticFilePath(segment, path.to_path_buf())
                    })?
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
        segment: StaticFileSegment,
        fixed_block_range_end: BlockNumber,
    ) {
        self.map.remove(&(fixed_block_range_end, segment));
    }

    /// Given a segment and block range it deletes the jar and all files associated with it.
    ///
    /// CAUTION: destructive. Deletes files on disk.
    pub fn delete_jar(
        &self,
        segment: StaticFileSegment,
        fixed_block_range: SegmentRangeInclusive,
    ) -> ProviderResult<()> {
        let key = (fixed_block_range.end(), segment);
        let jar = if let Some((_, jar)) = self.map.remove(&key) {
            jar.jar
        } else {
            let mut jar = NippyJar::<SegmentHeader>::load(
                &self.path.join(segment.filename(&fixed_block_range)),
            )
            .map_err(|e| ProviderError::NippyJar(e.to_string()))?;
            if self.load_filters {
                jar.load_filters().map_err(|e| ProviderError::NippyJar(e.to_string()))?;
            }
            jar
        };

        jar.delete().map_err(|e| ProviderError::NippyJar(e.to_string()))?;

        let mut segment_max_block = None;
        if fixed_block_range.start() > 0 {
            segment_max_block = Some(fixed_block_range.start() - 1)
        };
        self.update_index(segment, segment_max_block)?;

        Ok(())
    }

    /// Given a segment and block range it returns a cached
    /// [`StaticFileJarProvider`]. TODO(joshie): we should check the size and pop N if there's too
    /// many.
    fn get_or_create_jar_provider(
        &self,
        segment: StaticFileSegment,
        fixed_block_range: &SegmentRangeInclusive,
    ) -> ProviderResult<StaticFileJarProvider<'_>> {
        let key = (fixed_block_range.end(), segment);

        // Avoid using `entry` directly to avoid a write lock in the common case.
        let mut provider: StaticFileJarProvider<'_> = if let Some(jar) = self.map.get(&key) {
            jar.into()
        } else {
            let path = self.path.join(segment.filename(fixed_block_range));
            let mut jar =
                NippyJar::load(&path).map_err(|e| ProviderError::NippyJar(e.to_string()))?;
            if self.load_filters {
                jar.load_filters().map_err(|e| ProviderError::NippyJar(e.to_string()))?;
            }

            self.map.entry(key).insert(LoadedJar::new(jar)?).downgrade().into()
        };

        if let Some(metrics) = &self.metrics {
            provider = provider.with_metrics(metrics.clone());
        }
        Ok(provider)
    }

    /// Gets a static file segment's block range from the provider inner block
    /// index.
    fn get_segment_ranges_from_block(
        &self,
        segment: StaticFileSegment,
        block: u64,
    ) -> Option<SegmentRangeInclusive> {
        self.static_files_max_block
            .read()
            .get(&segment)
            .filter(|max| **max >= block)
            .map(|_| find_fixed_range(block))
    }

    /// Gets a static file segment's fixed block range from the provider inner
    /// transaction index.
    fn get_segment_ranges_from_transaction(
        &self,
        segment: StaticFileSegment,
        tx: u64,
    ) -> Option<SegmentRangeInclusive> {
        let static_files = self.static_files_tx_index.read();
        let segment_static_files = static_files.get(&segment)?;

        // It's more probable that the request comes from a newer tx height, so we iterate
        // the static_files in reverse.
        let mut static_files_rev_iter = segment_static_files.iter().rev().peekable();

        while let Some((tx_end, block_range)) = static_files_rev_iter.next() {
            if tx > *tx_end {
                // request tx is higher than highest static file tx
                return None
            }
            let tx_start = static_files_rev_iter.peek().map(|(tx_end, _)| *tx_end + 1).unwrap_or(0);
            if tx_start <= tx {
                return Some(find_fixed_range(block_range.end()))
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
        segment: StaticFileSegment,
        segment_max_block: Option<BlockNumber>,
    ) -> ProviderResult<()> {
        let mut max_block = self.static_files_max_block.write();
        let mut tx_index = self.static_files_tx_index.write();

        match segment_max_block {
            Some(segment_max_block) => {
                // Update the max block for the segment
                max_block.insert(segment, segment_max_block);
                let fixed_range = find_fixed_range(segment_max_block);

                let jar = NippyJar::<SegmentHeader>::load(
                    &self.path.join(segment.filename(&fixed_range)),
                )
                .map_err(|e| ProviderError::NippyJar(e.to_string()))?;

                // Updates the tx index by first removing all entries which have a higher
                // block_start than our current static file.
                if let Some(tx_range) = jar.user_header().tx_range() {
                    let tx_end = tx_range.end();

                    // Current block range has the same block start as `fixed_range``, but block end
                    // might be different if we are still filling this static file.
                    if let Some(current_block_range) = jar.user_header().block_range().copied() {
                        // Considering that `update_index` is called when we either append/truncate,
                        // we are sure that we are handling the latest data
                        // points.
                        //
                        // Here we remove every entry of the index that has a block start higher or
                        // equal than our current one. This is important in the case
                        // that we prune a lot of rows resulting in a file (and thus
                        // a higher block range) deletion.
                        tx_index
                            .entry(segment)
                            .and_modify(|index| {
                                index.retain(|_, block_range| {
                                    block_range.start() < fixed_range.start()
                                });
                                index.insert(tx_end, current_block_range);
                            })
                            .or_insert_with(|| BTreeMap::from([(tx_end, current_block_range)]));
                    }
                } else if tx_index.get(&segment).map(|index| index.len()) == Some(1) {
                    // Only happens if we unwind all the txs/receipts from the first static file.
                    // Should only happen in test scenarios.
                    if jar.user_header().expected_block_start() == 0 &&
                        matches!(
                            segment,
                            StaticFileSegment::Receipts | StaticFileSegment::Transactions
                        )
                    {
                        tx_index.remove(&segment);
                    }
                }

                // Update the cached provider.
                self.map.insert((fixed_range.end(), segment), LoadedJar::new(jar)?);

                // Delete any cached provider that no longer has an associated jar.
                self.map.retain(|(end, seg), _| !(*seg == segment && *end > fixed_range.end()));
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
        let mut max_block = self.static_files_max_block.write();
        let mut tx_index = self.static_files_tx_index.write();

        tx_index.clear();

        for (segment, ranges) in
            iter_static_files(&self.path).map_err(|e| ProviderError::NippyJar(e.to_string()))?
        {
            // Update last block for each segment
            if let Some((block_range, _)) = ranges.last() {
                max_block.insert(segment, block_range.end());
            }

            // Update tx -> block_range index
            for (block_range, tx_range) in ranges {
                if let Some(tx_range) = tx_range {
                    let tx_end = tx_range.end();

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

    /// Gets the highest static file block if it exists for a static file segment.
    pub fn get_highest_static_file_block(&self, segment: StaticFileSegment) -> Option<BlockNumber> {
        self.static_files_max_block.read().get(&segment).copied()
    }

    /// Gets the highest static file transaction.
    pub fn get_highest_static_file_tx(&self, segment: StaticFileSegment) -> Option<TxNumber> {
        self.static_files_tx_index
            .read()
            .get(&segment)
            .and_then(|index| index.last_key_value().map(|(last_tx, _)| *last_tx))
    }

    /// Gets the highest static file block for all segments.
    pub fn get_highest_static_files(&self) -> HighestStaticFiles {
        HighestStaticFiles {
            headers: self.get_highest_static_file_block(StaticFileSegment::Headers),
            receipts: self.get_highest_static_file_block(StaticFileSegment::Receipts),
            transactions: self.get_highest_static_file_block(StaticFileSegment::Transactions),
        }
    }

    /// Iterates through segment static_files in reverse order, executing a function until it
    /// returns some object. Useful for finding objects by [`TxHash`] or [`BlockHash`].
    pub fn find_static_file<T>(
        &self,
        segment: StaticFileSegment,
        func: impl Fn(StaticFileJarProvider<'_>) -> ProviderResult<Option<T>>,
    ) -> ProviderResult<Option<T>> {
        if let Some(highest_block) = self.get_highest_static_file_block(segment) {
            let mut range = find_fixed_range(highest_block);
            while range.end() > 0 {
                if let Some(res) = func(self.get_or_create_jar_provider(segment, &range)?)? {
                    return Ok(Some(res))
                }
                range = SegmentRangeInclusive::new(
                    range.start().saturating_sub(BLOCKS_PER_STATIC_FILE),
                    range.end().saturating_sub(BLOCKS_PER_STATIC_FILE),
                );
            }
        }

        Ok(None)
    }

    /// Fetches data within a specified range across multiple static files.
    ///
    /// This function iteratively retrieves data using `get_fn` for each item in the given range.
    /// It continues fetching until the end of the range is reached or the provided `predicate`
    /// returns false.
    pub fn fetch_range_with_predicate<T, F, P>(
        &self,
        segment: StaticFileSegment,
        range: Range<u64>,
        mut get_fn: F,
        mut predicate: P,
    ) -> ProviderResult<Vec<T>>
    where
        F: FnMut(&mut StaticFileCursor<'_>, u64) -> ProviderResult<Option<T>>,
        P: FnMut(&T) -> bool,
    {
        let get_provider = |start: u64| match segment {
            StaticFileSegment::Headers => {
                self.get_segment_provider_from_block(segment, start, None)
            }
            StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
                self.get_segment_provider_from_transaction(segment, start, None)
            }
        };

        let mut result = Vec::with_capacity((range.end - range.start).min(100) as usize);
        let mut provider = get_provider(range.start)?;
        let mut cursor = provider.cursor()?;

        // advances number in range
        'outer: for number in range {
            // The `retrying` flag ensures a single retry attempt per `number`. If `get_fn` fails to
            // access data in two different static files, it halts further attempts by returning
            // an error, effectively preventing infinite retry loops.
            let mut retrying = false;

            // advances static files if `get_fn` returns None
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
                        if retrying {
                            warn!(
                                target: "provider::static_file",
                                ?segment,
                                ?number,
                                "Could not find block or tx number on a range request"
                            );

                            let err = if segment.is_headers() {
                                ProviderError::MissingStaticFileBlock(segment, number)
                            } else {
                                ProviderError::MissingStaticFileTx(segment, number)
                            };
                            return Err(err)
                        }
                        provider = get_provider(number)?;
                        cursor = provider.cursor()?;
                        retrying = true;
                    }
                }
            }
        }

        Ok(result)
    }

    /// Fetches data within a specified range across multiple static files.
    ///
    /// Returns an iterator over the data
    pub fn fetch_range_iter<'a, T, F>(
        &'a self,
        segment: StaticFileSegment,
        range: Range<u64>,
        get_fn: F,
    ) -> ProviderResult<impl Iterator<Item = ProviderResult<T>> + 'a>
    where
        F: Fn(&mut StaticFileCursor<'_>, u64) -> ProviderResult<Option<T>> + 'a,
        T: std::fmt::Debug,
    {
        let get_provider = move |start: u64| match segment {
            StaticFileSegment::Headers => {
                self.get_segment_provider_from_block(segment, start, None)
            }
            StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
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

    /// Returns directory where static_files are located.
    pub fn directory(&self) -> &Path {
        &self.path
    }

    /// Retrieves data from the database or static file, wherever it's available.
    ///
    /// # Arguments
    /// * `segment` - The segment of the static file to check against.
    /// * `index_key` - Requested index key, usually a block or transaction number.
    /// * `fetch_from_static_file` - A closure that defines how to fetch the data from the static
    ///   file provider.
    /// * `fetch_from_database` - A closure that defines how to fetch the data from the database
    ///   when the static file doesn't contain the required data or is not available.
    pub fn get_with_static_file_or_database<T, FS, FD>(
        &self,
        segment: StaticFileSegment,
        number: u64,
        fetch_from_static_file: FS,
        fetch_from_database: FD,
    ) -> ProviderResult<Option<T>>
    where
        FS: Fn(&StaticFileProvider) -> ProviderResult<Option<T>>,
        FD: Fn() -> ProviderResult<Option<T>>,
    {
        // If there is, check the maximum block or transaction number of the segment.
        let static_file_upper_bound = match segment {
            StaticFileSegment::Headers => self.get_highest_static_file_block(segment),
            StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
                self.get_highest_static_file_tx(segment)
            }
        };

        if static_file_upper_bound
            .map_or(false, |static_file_upper_bound| static_file_upper_bound >= number)
        {
            return fetch_from_static_file(self)
        }
        fetch_from_database()
    }

    /// Gets data within a specified range, potentially spanning different static_files and
    /// database.
    ///
    /// # Arguments
    /// * `segment` - The segment of the static file to query.
    /// * `block_range` - The range of data to fetch.
    /// * `fetch_from_static_file` - A function to fetch data from the static_file.
    /// * `fetch_from_database` - A function to fetch data from the database.
    /// * `predicate` - A function used to evaluate each item in the fetched data. Fetching is
    ///   terminated when this function returns false, thereby filtering the data based on the
    ///   provided condition.
    pub fn get_range_with_static_file_or_database<T, P, FS, FD>(
        &self,
        segment: StaticFileSegment,
        mut block_or_tx_range: Range<u64>,
        fetch_from_static_file: FS,
        mut fetch_from_database: FD,
        mut predicate: P,
    ) -> ProviderResult<Vec<T>>
    where
        FS: Fn(&StaticFileProvider, Range<u64>, &mut P) -> ProviderResult<Vec<T>>,
        FD: FnMut(Range<u64>, P) -> ProviderResult<Vec<T>>,
        P: FnMut(&T) -> bool,
    {
        let mut data = Vec::new();

        // If there is, check the maximum block or transaction number of the segment.
        if let Some(static_file_upper_bound) = match segment {
            StaticFileSegment::Headers => self.get_highest_static_file_block(segment),
            StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
                self.get_highest_static_file_tx(segment)
            }
        } {
            if block_or_tx_range.start <= static_file_upper_bound {
                let end = block_or_tx_range.end.min(static_file_upper_bound + 1);
                data.extend(fetch_from_static_file(
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

    #[cfg(any(test, feature = "test-utils"))]
    /// Returns static_files directory
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Helper trait to manage different [`StaticFileProviderRW`] of an `Arc<StaticFileProvider`
pub trait StaticFileWriter {
    /// Returns a mutable reference to a [`StaticFileProviderRW`] of a [`StaticFileSegment`].
    fn get_writer(
        &self,
        block: BlockNumber,
        segment: StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_>>;

    /// Returns a mutable reference to a [`StaticFileProviderRW`] of the latest
    /// [`StaticFileSegment`].
    fn latest_writer(
        &self,
        segment: StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_>>;

    /// Commits all changes of all [`StaticFileProviderRW`] of all [`StaticFileSegment`].
    fn commit(&self) -> ProviderResult<()>;
}

impl StaticFileWriter for StaticFileProvider {
    fn get_writer(
        &self,
        block: BlockNumber,
        segment: StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_>> {
        tracing::trace!(target: "providers::static_file", ?block, ?segment, "Getting static file writer.");
        Ok(match self.writers.entry(segment) {
            DashMapEntry::Occupied(entry) => entry.into_ref(),
            DashMapEntry::Vacant(entry) => {
                let writer = StaticFileProviderRW::new(
                    segment,
                    block,
                    Arc::downgrade(&self.0),
                    self.metrics.clone(),
                )?;
                entry.insert(writer)
            }
        })
    }

    fn latest_writer(
        &self,
        segment: StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_>> {
        self.get_writer(self.get_highest_static_file_block(segment).unwrap_or_default(), segment)
    }

    fn commit(&self) -> ProviderResult<()> {
        for mut writer in self.writers.iter_mut() {
            writer.commit()?;
        }
        Ok(())
    }
}

impl HeaderProvider for StaticFileProvider {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.find_static_file(StaticFileSegment::Headers, |jar_provider| {
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
        self.get_segment_provider_from_block(StaticFileSegment::Headers, num, None)?
            .header_by_number(num)
    }

    fn header_td(&self, block_hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.find_static_file(StaticFileSegment::Headers, |jar_provider| {
            Ok(jar_provider
                .cursor()?
                .get_two::<HeaderMask<CompactU256, BlockHash>>(block_hash.into())?
                .and_then(|(td, hash)| (&hash == block_hash).then_some(td.0)))
        })
    }

    fn header_td_by_number(&self, num: BlockNumber) -> ProviderResult<Option<U256>> {
        self.get_segment_provider_from_block(StaticFileSegment::Headers, num, None)?
            .header_td_by_number(num)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.fetch_range_with_predicate(
            StaticFileSegment::Headers,
            to_range(range),
            |cursor, number| cursor.get_one::<HeaderMask<Header>>(number.into()),
            |_| true,
        )
    }

    fn sealed_header(&self, num: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.get_segment_provider_from_block(StaticFileSegment::Headers, num, None)?
            .sealed_header(num)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.fetch_range_with_predicate(
            StaticFileSegment::Headers,
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

impl BlockHashReader for StaticFileProvider {
    fn block_hash(&self, num: u64) -> ProviderResult<Option<B256>> {
        self.get_segment_provider_from_block(StaticFileSegment::Headers, num, None)?.block_hash(num)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.fetch_range_with_predicate(
            StaticFileSegment::Headers,
            start..end,
            |cursor, number| cursor.get_one::<HeaderMask<BlockHash>>(number.into()),
            |_| true,
        )
    }
}

impl ReceiptProvider for StaticFileProvider {
    fn receipt(&self, num: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.get_segment_provider_from_transaction(StaticFileSegment::Receipts, num, None)?
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
            StaticFileSegment::Receipts,
            to_range(range),
            |cursor, number| cursor.get_one::<ReceiptMask<Receipt>>(number.into()),
            |_| true,
        )
    }
}

impl TransactionsProviderExt for StaticFileProvider {
    fn transaction_hashes_by_range(
        &self,
        tx_range: Range<TxNumber>,
    ) -> ProviderResult<Vec<(TxHash, TxNumber)>> {
        let tx_range_size = (tx_range.end - tx_range.start) as usize;

        // Transactions are different size, so chunks will not all take the same processing time. If
        // chunks are too big, there will be idle threads waiting for work. Choosing an
        // arbitrary smaller value to make sure it doesn't happen.
        let chunk_size = 100;
        let mut channels = Vec::new();

        // iterator over the chunks
        let chunks = (tx_range.start..tx_range.end)
            .step_by(chunk_size)
            .map(|start| start..std::cmp::min(start + chunk_size as u64, tx_range.end));

        for chunk_range in chunks {
            let (channel_tx, channel_rx) = mpsc::channel();
            channels.push(channel_rx);

            let manager = self.clone();

            // Spawn the task onto the global rayon pool
            // This task will send the results through the channel after it has calculated
            // the hash.
            rayon::spawn(move || {
                let mut rlp_buf = Vec::with_capacity(128);
                let _ = manager.fetch_range_with_predicate(
                    StaticFileSegment::Transactions,
                    chunk_range,
                    |cursor, number| {
                        Ok(cursor
                            .get_one::<TransactionMask<TransactionSignedNoHash>>(number.into())?
                            .map(|transaction| {
                                rlp_buf.clear();
                                let _ = channel_tx
                                    .send(calculate_hash((number, transaction), &mut rlp_buf));
                            }))
                    },
                    |_| true,
                );
            });
        }

        let mut tx_list = Vec::with_capacity(tx_range_size);

        // Iterate over channels and append the tx hashes unsorted
        for channel in channels {
            while let Ok(tx) = channel.recv() {
                let (tx_hash, tx_id) = tx.map_err(|boxed| *boxed)?;
                tx_list.push((tx_hash, tx_id));
            }
        }

        Ok(tx_list)
    }
}

impl TransactionsProvider for StaticFileProvider {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.find_static_file(StaticFileSegment::Transactions, |jar_provider| {
            let mut cursor = jar_provider.cursor()?;
            if cursor
                .get_one::<TransactionMask<TransactionSignedNoHash>>((&tx_hash).into())?
                .and_then(|tx| (tx.hash() == tx_hash).then_some(tx))
                .is_some()
            {
                Ok(cursor.number())
            } else {
                Ok(None)
            }
        })
    }

    fn transaction_by_id(&self, num: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.get_segment_provider_from_transaction(StaticFileSegment::Transactions, num, None)?
            .transaction_by_id(num)
    }

    fn transaction_by_id_no_hash(
        &self,
        num: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.get_segment_provider_from_transaction(StaticFileSegment::Transactions, num, None)?
            .transaction_by_id_no_hash(num)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        self.find_static_file(StaticFileSegment::Transactions, |jar_provider| {
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
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.fetch_range_with_predicate(
            StaticFileSegment::Transactions,
            to_range(range),
            |cursor, number| {
                cursor.get_one::<TransactionMask<TransactionSignedNoHash>>(number.into())
            },
            |_| true,
        )
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        let txes = self.transactions_by_tx_range(range)?;
        TransactionSignedNoHash::recover_signers(&txes, txes.len())
            .ok_or(ProviderError::SenderRecoveryError)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(self.transaction_by_id_no_hash(id)?.and_then(|tx| tx.recover_signer()))
    }
}

/* Cannot be successfully implemented but must exist for trait requirements */

impl BlockNumReader for StaticFileProvider {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }
}

impl BlockReader for StaticFileProvider {
    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Block>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn block(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }
}

impl WithdrawalsProvider for StaticFileProvider {
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        // Required data not present in static_files
        Err(ProviderError::UnsupportedProvider)
    }
}

impl StatsReader for StaticFileProvider {
    fn count_entries<T: Table>(&self) -> ProviderResult<usize> {
        match T::NAME {
            tables::CanonicalHeaders::NAME |
            tables::Headers::NAME |
            tables::HeaderTerminalDifficulties::NAME => Ok(self
                .get_highest_static_file_block(StaticFileSegment::Headers)
                .map(|block| block + 1)
                .unwrap_or_default()
                as usize),
            tables::Receipts::NAME => Ok(self
                .get_highest_static_file_tx(StaticFileSegment::Receipts)
                .map(|receipts| receipts + 1)
                .unwrap_or_default() as usize),
            tables::Transactions::NAME => Ok(self
                .get_highest_static_file_tx(StaticFileSegment::Transactions)
                .map(|txs| txs + 1)
                .unwrap_or_default() as usize),
            _ => Err(ProviderError::UnsupportedProvider),
        }
    }
}

/// Calculates the tx hash for the given transaction and its id.
#[inline]
fn calculate_hash(
    entry: (TxNumber, TransactionSignedNoHash),
    rlp_buf: &mut Vec<u8>,
) -> Result<(B256, TxNumber), Box<ProviderError>> {
    let (tx_id, tx) = entry;
    tx.transaction.encode_with_signature(&tx.signature, rlp_buf, false);
    Ok((keccak256(rlp_buf), tx_id))
}
