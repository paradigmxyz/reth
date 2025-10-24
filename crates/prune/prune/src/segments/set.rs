use crate::segments::Segment;

/// Collection of [`Segment`]. Thread-safe, allocated on the heap.
#[derive(Debug)]
pub struct SegmentSet<Provider> {
    inner: Vec<Box<dyn Segment<Provider>>>,
}

impl<Provider> SegmentSet<Provider> {
    /// Returns empty [`SegmentSet`] collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds new [`Segment`] to collection.
    pub fn segment<S: Segment<Provider> + 'static>(mut self, segment: S) -> Self {
        self.inner.push(Box::new(segment));
        self
    }

    /// Adds new [Segment] to collection if it's [Some].
    pub fn segment_opt<S: Segment<Provider> + 'static>(self, segment: Option<S>) -> Self {
        if let Some(segment) = segment {
            return self.segment(segment)
        }
        self
    }

    /// Consumes [`SegmentSet`] and returns a [Vec].
    pub fn into_vec(self) -> Vec<Box<dyn Segment<Provider>>> {
        self.inner
    }
}

impl<Provider> Default for SegmentSet<Provider> {
    fn default() -> Self {
        Self { inner: Vec::new() }
    }
}
