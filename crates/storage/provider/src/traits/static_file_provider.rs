use alloy_primitives::BlockNumber;
use reth_errors::ProviderResult;
use reth_static_file_types::StaticFileSegment;
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

    /// Deletes all static files of the segment below the given block, like
    /// [`StaticFileProvider::delete_segment_below_block`].
    ///
    /// CAUTION: destructive. Deletes files on disk.
    ///
    /// A provider that owns a database transaction must defer the deletion until its commit is
    /// durable, so that a dropped provider or a crash never leaves a checkpoint behind the
    /// deleted files. A provider without a transaction deletes the files immediately.
    fn delete_static_files_below_block_on_commit(
        &self,
        segment: StaticFileSegment,
        block: BlockNumber,
    ) -> ProviderResult<()> {
        self.static_file_provider().delete_segment_below_block(segment, block)?;
        Ok(())
    }
}
