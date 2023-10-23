//! Snapshot primitives.

mod compression;
mod filters;
mod segment;

/// Default snapshot block count.
pub const SNAPSHOT_BLOCK_NUMBER_CHUNKS: u64 = 500_000;

pub use compression::Compression;
pub use filters::{Filters, InclusionFilter, PerfectHashingFunction};
pub use segment::{SegmentHeader, SnapshotSegment};
