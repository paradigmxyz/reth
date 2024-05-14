use std::{marker::PhantomData, sync::Arc};

use reth_db::database::Database;
use reth_primitives::{PruneMode, PrunePurpose, StaticFileSegment};
use reth_provider::providers::StaticFileProvider;

use crate::{segments, Segment};

/// A mutable iterator over a ring of tables. Returns `None` after the first cycle.
#[derive(Debug)]
pub struct SegmentIter<'a, T> {
    ring: &'a mut T,
}

/// Cycles prunable segments.
pub trait CycleSegments {
    type Db: Database;
    type TableRef: Eq;

    /// Returns the starting position in the ring. This is needed for counting cycles in the ring.
    fn start_table(&self) -> Self::TableRef;
    /// Returns the current table in the ring. This table has not been pruned yet in the current
    /// cycle.
    fn current_table(&self) -> Self::TableRef;
    /// Returns the table corresponding to the [`Segment`] most recently returned by
    /// [`next_segment`](CycleSegments::next_segment).
    fn prev_table(&self) -> Option<Self::TableRef>;
    /// Returns the next position in the ring. This table will be pruned after the current table.
    fn next_table(&self) -> Self::TableRef;
    /// Returns the next [`Segment`] to prune, if any entries to prune for the current table.
    #[allow(clippy::type_complexity)]
    fn next_segment(&mut self) -> Option<(Arc<dyn Segment<Self::Db>>, PrunePurpose)>;
    /// Returns an iterator cycling once over the ring of tables. Yields an item for each table,
    /// either a segment or `None`. Advances the current position in the ring.
    fn next_cycle(
        &mut self,
    ) -> impl Iterator<Item = Option<(Arc<dyn Segment<Self::Db>>, PrunePurpose)>>
    where
        Self: Sized,
    {
        SegmentIter { ring: self }
    }
    /// Returns an iterator over prunable segments. See [`next_cycle`](CycleSegments::next_cycle).
    fn iter(&mut self) -> impl Iterator<Item = (Arc<dyn Segment<Self::Db>>, PrunePurpose)>
    where
        Self: Sized,
    {
        self.next_cycle().filter(|segment| segment.is_some()).flatten()
    }
}

impl<'a, T> Iterator for SegmentIter<'a, T>
where
    T: CycleSegments,
{
    type Item = Option<(Arc<dyn Segment<<T as CycleSegments>::Db>>, PrunePurpose)>;

    /// Returns next prunable segment in ring, or `None` if iterator has walked one cycle.
    fn next(&mut self) -> Option<Self::Item> {
        // return after one cycle
        if self.ring.prev_table().is_some() && self.ring.current_table() == self.ring.start_table()
        {
            return None
        }

        Some(self.ring.next_segment())
    }
}

/// Opaque reference to a table.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TableRef {
    StaticFiles(StaticFileTableRef),
    Drop(usize),
}

impl Default for TableRef {
    fn default() -> Self {
        Self::StaticFiles(StaticFileTableRef::default())
    }
}

/// A ring over prunable tables.
#[derive(Debug)]
pub struct TableRing<DB> {
    start: TableRef,
    current: TableRef,
    prev: Option<TableRef>,
    segments: Vec<Arc<dyn Segment<DB>>>,
    static_file_start: StaticFileTableRef,
    static_file_ring: StaticFileTableRing<DB>,
}

impl<DB> TableRing<DB> {
    pub fn new(
        provider: StaticFileProvider,
        start: TableRef,
        segments: Vec<Arc<dyn Segment<DB>>>,
    ) -> Result<Self, &'static str> {
        let static_file_start = match start {
            TableRef::StaticFiles(table_ref) => table_ref,
            _ => StaticFileTableRef::default(),
        };

        if let TableRef::Drop(index) = start {
            if segments.is_empty() || index > segments.len() - 1 {
                return Err("segments index out of bounds")
            }
        }

        Ok(Self {
            start,
            current: start,
            prev: None,
            segments,
            static_file_start,
            static_file_ring: StaticFileTableRing::new(provider, static_file_start),
        })
    }
}

impl<DB> CycleSegments for TableRing<DB>
where
    DB: Database,
{
    type Db = <StaticFileTableRing<DB> as CycleSegments>::Db;
    type TableRef = TableRef;

    fn start_table(&self) -> Self::TableRef {
        self.start
    }

    fn current_table(&self) -> Self::TableRef {
        self.current
    }

    fn prev_table(&self) -> Option<Self::TableRef> {
        self.prev
    }

    fn next_table(&self) -> Self::TableRef {
        let Self { current, static_file_start, static_file_ring, segments, .. } = self;

        match current {
            TableRef::StaticFiles(_) => {
                // static files ring nested in this ring, so is one step ahead
                let next = static_file_ring.current_table();
                if next == *static_file_start && !segments.is_empty() {
                    TableRef::Drop(0)
                } else {
                    TableRef::StaticFiles(next)
                }
            }
            TableRef::Drop(index) => {
                if *index == segments.len() - 1 {
                    // start next cycle
                    TableRef::StaticFiles(StaticFileTableRef::default())
                } else {
                    TableRef::Drop(*index + 1)
                }
            }
        }
    }

    fn next_segment(&mut self) -> Option<(Arc<dyn Segment<Self::Db>>, PrunePurpose)> {
        let Self { current, segments, .. } = self;

        let segment = match current {
            TableRef::StaticFiles(_) => self.static_file_ring.next_cycle().next().flatten(),
            TableRef::Drop(index) => Some((segments[*index].clone(), PrunePurpose::User)),
        };

        self.prev = Some(*current);
        self.current = self.next_table();

        segment
    }
}

/// Opaque reference to a static file table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum StaticFileTableRef {
    #[default]
    Headers,
    Transactions,
    Receipts,
}

/// A ring over static file tables.
///
/// Iterator that returns pre-configured segments that needs to be pruned according to the highest
/// static files for [PruneSegment::Transactions](reth_primitives::PruneSegment::Transactions),
/// [PruneSegment::Headers](reth_primitives::PruneSegment::Headers) and
/// [PruneSegment::Receipts](reth_primitives::PruneSegment::Receipts).
#[derive(Debug)]
pub struct StaticFileTableRing<DB> {
    provider: StaticFileProvider,
    start: StaticFileTableRef,
    current: StaticFileTableRef,
    prev: Option<StaticFileTableRef>,
    _phantom: PhantomData<DB>,
}

impl<DB> StaticFileTableRing<DB> {
    pub fn new(provider: StaticFileProvider, start: StaticFileTableRef) -> Self {
        Self { provider, start, current: start, prev: None, _phantom: PhantomData }
    }
}

impl<DB> CycleSegments for StaticFileTableRing<DB>
where
    DB: Database,
{
    type Db = DB;
    type TableRef = StaticFileTableRef;

    fn start_table(&self) -> Self::TableRef {
        self.start
    }

    fn current_table(&self) -> Self::TableRef {
        self.current
    }

    fn prev_table(&self) -> Option<Self::TableRef> {
        self.prev
    }

    fn next_table(&self) -> Self::TableRef {
        use StaticFileTableRef::*;

        match self.current {
            Headers => Transactions,
            Transactions => Receipts,
            Receipts => Headers,
        }
    }

    fn next_segment(&mut self) -> Option<(Arc<dyn Segment<Self::Db>>, PrunePurpose)> {
        let Self { provider, current, .. } = self;

        let segment = match current {
            StaticFileTableRef::Headers => {
                provider.get_highest_static_file_block(StaticFileSegment::Headers).map(|to_block| {
                    Arc::new(segments::Headers::new(PruneMode::before_inclusive(to_block)))
                        as Arc<dyn Segment<DB>>
                })
            }
            StaticFileTableRef::Transactions => provider
                .get_highest_static_file_block(StaticFileSegment::Transactions)
                .map(|to_block| {
                    Arc::new(segments::Transactions::new(PruneMode::before_inclusive(to_block)))
                        as Arc<dyn Segment<DB>>
                }),
            StaticFileTableRef::Receipts => provider
                .get_highest_static_file_block(StaticFileSegment::Receipts)
                .map(|to_block| {
                    Arc::new(segments::Receipts::new(PruneMode::before_inclusive(to_block)))
                        as Arc<dyn Segment<DB>>
                }),
        };

        self.prev = Some(*current);
        self.current = self.next_table();

        segment.map(|sgmnt| (sgmnt, PrunePurpose::StaticFile))
    }
}

#[cfg(test)]
mod test {
    use rand::Rng;
    use reth_db::{
        tables,
        test_utils::{create_test_rw_db, create_test_static_files_dir, TempDatabase},
        transaction::DbTxMut,
        DatabaseEnv,
    };
    use reth_primitives::{PruneModes, B256, MAINNET, U256};
    use reth_provider::{ProviderFactory, StaticFileProviderFactory, StaticFileWriter};

    use crate::segments::SegmentSet;

    use super::*;

    #[test]
    fn cycle_with_one_static_file_segment() {
        let db = create_test_rw_db();
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let provider_factory = ProviderFactory::new(db, MAINNET.clone(), static_dir_path)
            .expect("create provide factory with static_files");

        let provider_rw = provider_factory.provider_rw().unwrap();
        let tx = provider_rw.tx_ref();
        tx.put::<tables::HeaderNumbers>(B256::default(), 0).unwrap();
        tx.put::<tables::BlockBodyIndices>(0, Default::default()).unwrap();

        let segments: Vec<Arc<dyn Segment<TempDatabase<DatabaseEnv>>>> =
            SegmentSet::from_prune_modes(PruneModes::all()).into_vec();
        let segments_len = segments.len();

        let static_file_provider = provider_factory.static_file_provider();

        let (header, block_hash) = MAINNET.sealed_genesis_header().split();
        static_file_provider
            .latest_writer(StaticFileSegment::Headers)
            .expect("get static file writer for headers")
            .append_header(header, U256::ZERO, block_hash)
            .unwrap();

        static_file_provider.commit().unwrap();

        let mut ring: TableRing<_> =
            TableRing::new(static_file_provider, TableRef::default(), segments).unwrap();

        let total_segments = ring.iter().count();

        // + 1 non-empty static file segments
        assert_eq!(segments_len + 1, total_segments);
        // back at start table
        assert_eq!(TableRef::default(), ring.current_table());
        assert_eq!(TableRef::Drop(segments_len - 1), ring.prev_table().unwrap());
    }

    #[test]
    fn cycle_start_at_headers() {
        let db = create_test_rw_db();
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let provider_factory = ProviderFactory::new(db, MAINNET.clone(), static_dir_path)
            .expect("create provide factory with static_files");

        let segments: Vec<Arc<dyn Segment<TempDatabase<DatabaseEnv>>>> =
            SegmentSet::from_prune_modes(PruneModes::all()).into_vec();
        let segments_len = segments.len();

        let mut ring: TableRing<_> =
            TableRing::new(provider_factory.static_file_provider(), TableRef::default(), segments)
                .unwrap();

        let cycle = SegmentIter { ring: &mut ring };
        let total_segments = cycle.count();

        // + 3 static file segments
        assert_eq!(segments_len + 3, total_segments);
        // back at start table
        assert_eq!(TableRef::default(), ring.current_table());
        assert_eq!(TableRef::Drop(segments_len - 1), ring.prev_table().unwrap());
    }

    fn expected_prev_table(start_index: usize) -> TableRef {
        if start_index == 0 {
            TableRef::StaticFiles(StaticFileTableRef::Receipts)
        } else {
            TableRef::Drop(start_index - 1)
        }
    }

    #[test]
    fn cycle_start_random_non_static_files_segment() {
        let db = create_test_rw_db();
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let provider_factory = ProviderFactory::new(db, MAINNET.clone(), static_dir_path)
            .expect("create provide factory with static_files");

        let segments: Vec<Arc<dyn Segment<TempDatabase<DatabaseEnv>>>> =
            SegmentSet::from_prune_modes(PruneModes::all()).into_vec();
        let segments_len = segments.len();

        let index = rand::thread_rng().gen_range(0..segments_len);
        let start = TableRef::Drop(index);
        let mut ring: TableRing<_> =
            TableRing::new(provider_factory.static_file_provider(), start, segments).unwrap();

        let cycle = SegmentIter { ring: &mut ring };
        let total_segments = cycle.count();

        // + 3 static file segments
        assert_eq!(segments_len + 3, total_segments);
        // back at start table
        assert_eq!(start, ring.current_table());
        assert_eq!(expected_prev_table(index), ring.prev_table().unwrap());
    }

    fn random_static_file_table_ref() -> StaticFileTableRef {
        use StaticFileTableRef::*;

        match rand::thread_rng().gen_range(0..3) {
            0 => Headers,
            1 => Transactions,
            _ => Receipts,
        }
    }

    fn expected_prev_static_files_table(table_ref: StaticFileTableRef) -> StaticFileTableRef {
        use StaticFileTableRef::*;

        match table_ref {
            Headers => Receipts,
            Transactions => Headers,
            Receipts => Transactions,
        }
    }

    #[test]
    fn cycle_static_files_start_random_segment() {
        let db = create_test_rw_db();
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let provider_factory = ProviderFactory::new(db, MAINNET.clone(), static_dir_path)
            .expect("create provide factory with static_files");

        let start = random_static_file_table_ref();
        let mut ring: StaticFileTableRing<TempDatabase<DatabaseEnv>> =
            StaticFileTableRing::new(provider_factory.static_file_provider(), start);

        let cycle = SegmentIter { ring: &mut ring };
        let total_segments = cycle.count();

        // 3 static file segments
        assert_eq!(3, total_segments);
        // back at start table
        assert_eq!(start, ring.current_table());
        assert_eq!(expected_prev_static_files_table(start), ring.prev_table().unwrap());
    }
}
