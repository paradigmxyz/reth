use std::{marker::PhantomData, sync::Arc};

use reth_db::database::Database;
use reth_primitives::{PruneMode, PrunePurpose, StaticFileSegment};
use reth_provider::providers::StaticFileProvider;

use crate::{segments, Segment};

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
    fn prev_table(&self) -> Self::TableRef;
    /// Returns the next position in the ring. This table will be pruned after the current table.
    fn next_table(&self) -> Self::TableRef;
    /// Returns the next [`Segment`] to prune, if any entries to prune for the current table.
    #[allow(clippy::type_complexity)]
    fn next_segment(&mut self) -> Option<(Arc<dyn Segment<Self::Db>>, PrunePurpose)>;
}

macro_rules! cycle_iterator_impl {
    ($ty:ty) => {
        impl<DB> Iterator for &mut $ty
        where
            DB: Database,
        {
            type Item = (Arc<dyn Segment<DB>>, PrunePurpose);

            /// Returns next prunable segment in ring, or `None` if iterator has walked one cycle.
            fn next(&mut self) -> Option<Self::Item> {
                loop {
                    if self.prev_table() == self.start_table() {
                        return None
                    }

                    let segment = self.next_segment();

                    // table is not completely pruned.
                    if segment.is_some() {
                        return segment
                    }
                }
            }
        }
    };
}

/// Opaque reference to a table.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TableRef {
    StaticFiles(StaticFileTableRef),
    Garbage(usize),
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
    prev: TableRef,
    segments: Vec<Arc<dyn Segment<DB>>>,
    static_file_start: StaticFileTableRef,
    static_file_ring: StaticFileTableRing<DB>,
}

cycle_iterator_impl!(TableRing<DB>);

impl<DB> TableRing<DB> {
    pub fn new(
        provider: StaticFileProvider,
        start: TableRef,
        segments: Vec<Arc<dyn Segment<DB>>>,
    ) -> Self {
        let static_file_start = match start {
            TableRef::StaticFiles(table_ref) => table_ref,
            _ => StaticFileTableRef::default(),
        };

        Self {
            start,
            current: start,
            prev: start,
            segments,
            static_file_start,
            static_file_ring: StaticFileTableRing::new(provider, static_file_start),
        }
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

    fn prev_table(&self) -> Self::TableRef {
        self.prev
    }

    fn next_table(&self) -> Self::TableRef {
        let Self { current, static_file_start, static_file_ring, segments, .. } = self;

        match current {
            TableRef::StaticFiles(_) => {
                let next = static_file_ring.next_table();
                if next == *static_file_start {
                    TableRef::Garbage(0)
                } else {
                    TableRef::StaticFiles(next)
                }
            }
            TableRef::Garbage(index) => {
                if *index == segments.len() - 2 {
                    // start next cycle
                    TableRef::StaticFiles(StaticFileTableRef::default())
                } else {
                    TableRef::Garbage(*index + 1)
                }
            }
        }
    }

    fn next_segment(&mut self) -> Option<(Arc<dyn Segment<Self::Db>>, PrunePurpose)> {
        let Self { current, segments, .. } = self;

        let segment = match current {
            TableRef::StaticFiles(_) => (&mut self.static_file_ring).next(),
            TableRef::Garbage(index) => Some((segments[*index].clone(), PrunePurpose::User)),
        };

        self.prev = *current;
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
    prev: StaticFileTableRef,
    _phantom: PhantomData<DB>,
}

cycle_iterator_impl!(StaticFileTableRing<DB>);

impl<DB> StaticFileTableRing<DB> {
    pub fn new(provider: StaticFileProvider, start: StaticFileTableRef) -> Self {
        Self { provider, start, current: start, prev: start, _phantom: PhantomData }
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

    fn prev_table(&self) -> Self::TableRef {
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

        self.prev = *current;
        self.current = self.next_table();

        segment.map(|sgmnt| (sgmnt, PrunePurpose::StaticFile))
    }
}
