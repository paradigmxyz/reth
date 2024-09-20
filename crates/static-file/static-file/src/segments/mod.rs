//! `StaticFile` segment implementations and utilities.

mod transactions;
pub use transactions::Transactions;

mod headers;
pub use headers::Headers;

mod receipts;
pub use receipts::Receipts;

use alloy_primitives::BlockNumber;
use reth_provider::providers::StaticFileProvider;
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// A segment represents moving some portion of the data to static files.
pub trait Segment<Provider>: Send + Sync {
    /// Returns the [`StaticFileSegment`].
    fn segment(&self) -> StaticFileSegment;

    /// Move data to static files for the provided block range. [`StaticFileProvider`] will handle
    /// the management of and writing to files.
    fn copy_to_static_files(
        &self,
        provider: Provider,
        static_file_provider: StaticFileProvider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()>;
}
