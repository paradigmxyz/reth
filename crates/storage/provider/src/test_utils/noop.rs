//! Additional testing support for `NoopProvider`.

use crate::{providers::StaticFileProvider, StaticFileProviderFactory};
use reth_primitives_traits::NodePrimitives;
use std::path::PathBuf;

/// Re-exported for convenience
pub use reth_storage_api::noop::NoopProvider;

impl<C: Send + Sync, N: NodePrimitives> StaticFileProviderFactory for NoopProvider<C, N> {
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives> {
        StaticFileProvider::read_only(PathBuf::default(), false).unwrap()
    }
}
