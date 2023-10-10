use crate::segments::Segment;
use reth_db::database::Database;
use std::sync::Arc;

#[derive(Debug)]
pub struct SegmentSet<DB: Database> {
    inner: Vec<Arc<dyn Segment<DB>>>,
}

impl<DB: Database> SegmentSet<DB> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_segment<S: Segment<DB> + 'static>(mut self, segment: S) -> Self {
        self.inner.push(Arc::new(segment));
        self
    }

    pub fn into_vec(self) -> Vec<Arc<dyn Segment<DB>>> {
        self.inner
    }
}

impl<DB: Database> Default for SegmentSet<DB> {
    fn default() -> Self {
        Self { inner: Vec::new() }
    }
}
