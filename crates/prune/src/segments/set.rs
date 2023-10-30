use crate::segments::Segment;
use reth_db::database::Database;
use std::sync::Arc;

/// Collection of [Segment]. Thread-safe, allocated on the heap.
#[derive(Debug)]
pub struct SegmentSet<DB: Database> {
    inner: Vec<Arc<dyn Segment<DB>>>,
}

impl<DB: Database> SegmentSet<DB> {
    /// Returns empty [SegmentSet] collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds new [Segment] to collection.
    pub fn segment<S: Segment<DB> + 'static>(mut self, segment: S) -> Self {
        self.inner.push(Arc::new(segment));
        self
    }

    /// Adds new [Segment] to collection if it's [Some].
    pub fn segment_opt<S: Segment<DB> + 'static>(self, segment: Option<S>) -> Self {
        if let Some(segment) = segment {
            return self.segment(segment)
        }
        self
    }

    /// Consumes [SegmentSet] and returns a [Vec].
    pub fn into_vec(self) -> Vec<Arc<dyn Segment<DB>>> {
        self.inner
    }
}

impl<DB: Database> Default for SegmentSet<DB> {
    fn default() -> Self {
        Self { inner: Vec::new() }
    }
}
