use alloy_primitives::BlockNumber;
use reth_errors::ProviderResult;
use reth_static_file_types::{SegmentHeader, StaticFileSegment};
use reth_storage_api::NodePrimitivesProvider;

use crate::providers::{StaticFileProvider, StaticFileProviderRWRefMut};

/// Static file provider factory.
pub trait StaticFileProviderFactory: NodePrimitivesProvider {
    /// Create new instance of static file provider.
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives>;

    /// Returns a mutable reference to a
    /// [`StaticFileProviderRW`](`crate::providers::StaticFileProviderRW`) of a
    /// [`StaticFileSegment`].
    fn get_static_file_writer(
        &self,
        block: BlockNumber,
        segment: StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_, Self::Primitives>>;

    /// Deletes the jars [`StaticFileProvider::delete_segment_below_block`] deletes, once this
    /// provider's writes are committed, and returns their headers.
    ///
    /// A database provider queues the deletion and runs it after its commit has written the
    /// `RocksDB` batches and the database transaction, so the prune checkpoint that covers the
    /// jars is durable before they go. Dropping it without committing drops the deletion. Other
    /// providers delete at once.
    fn delete_segment_below_block_after_commit(
        &self,
        segment: StaticFileSegment,
        block: BlockNumber,
    ) -> ProviderResult<Vec<SegmentHeader>> {
        self.static_file_provider().delete_segment_below_block(segment, block)
    }

    /// Deletes every jar of the segment, as [`StaticFileProvider::delete_segment`] does, once this
    /// provider's writes are committed, and returns their headers.
    ///
    /// Queued like [`Self::delete_segment_below_block_after_commit`].
    fn delete_segment_after_commit(
        &self,
        segment: StaticFileSegment,
    ) -> ProviderResult<Vec<SegmentHeader>> {
        self.static_file_provider().delete_segment(segment)
    }
}
