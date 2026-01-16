//! Additional testing support for `NoopProvider`.

use crate::{
    providers::{RocksDBProvider, StaticFileProvider, StaticFileProviderRWRefMut},
    RocksDBProviderFactory, StaticFileProviderFactory,
};
use reth_errors::{ProviderError, ProviderResult};
use reth_primitives_traits::NodePrimitives;
use std::path::PathBuf;

/// Re-exported for convenience
pub use reth_storage_api::noop::NoopProvider;

impl<C: Send + Sync, N: NodePrimitives> StaticFileProviderFactory for NoopProvider<C, N> {
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives> {
        StaticFileProvider::read_only(PathBuf::default(), false).unwrap()
    }

    fn get_static_file_writer(
        &self,
        _block: alloy_primitives::BlockNumber,
        _segment: reth_static_file_types::StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_, Self::Primitives>> {
        Err(ProviderError::ReadOnlyStaticFileAccess)
    }
}

impl<C: Send + Sync, N: NodePrimitives> RocksDBProviderFactory for NoopProvider<C, N> {
    fn rocksdb_provider(&self) -> RocksDBProvider {
        RocksDBProvider::builder(PathBuf::default()).with_default_tables().build().unwrap()
    }

    #[cfg(all(unix, feature = "rocksdb"))]
    fn set_pending_rocksdb_batch(&self, _batch: rocksdb::WriteBatchWithTransaction<true>) {
        // No-op for NoopProvider
    }
}
