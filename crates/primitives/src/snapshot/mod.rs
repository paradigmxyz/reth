//! Snapshot primitives.

mod compression;
mod filters;
mod segment;

pub use compression::Compression;
pub use filters::{Filters, InclusionFilter, PerfectHashingFunction};
pub use segment::{SegmentHeader, SnapshotSegment};

/// Default snapshot block count.
pub const BLOCKS_PER_SNAPSHOT: u64 = 500_000;
