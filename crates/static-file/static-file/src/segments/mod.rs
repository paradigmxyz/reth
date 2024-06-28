//! `StaticFile` segment implementations and utilities.

mod transactions;
pub use transactions::Transactions;

mod headers;
pub use headers::Headers;

mod receipts;
pub use receipts::Receipts;

use alloy_primitives::BlockNumber;
use reth_db::{RawKey, RawTable};
use reth_db_api::{cursor::DbCursorRO, database::Database, table::Table, transaction::DbTx};
use reth_nippy_jar::NippyJar;
use reth_provider::{
    providers::StaticFileProvider, DatabaseProviderRO, ProviderError, TransactionsProviderExt,
};
use reth_static_file_types::{
    find_fixed_range, Compression, Filters, InclusionFilter, PerfectHashingFunction, SegmentConfig,
    SegmentHeader, StaticFileSegment,
};
use reth_storage_errors::provider::ProviderResult;
use std::{ops::RangeInclusive, path::Path};

pub(crate) type Rows<const COLUMNS: usize> = [Vec<Vec<u8>>; COLUMNS];

/// A segment represents moving some portion of the data to static files.
pub trait Segment<DB: Database>: Send + Sync {
    /// Returns the [`StaticFileSegment`].
    fn segment(&self) -> StaticFileSegment;

    /// Move data to static files for the provided block range. [`StaticFileProvider`] will handle
    /// the management of and writing to files.
    fn copy_to_static_files(
        &self,
        provider: DatabaseProviderRO<DB>,
        static_file_provider: StaticFileProvider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()>;
}