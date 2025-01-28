use reth_db_api::table::Table;
use reth_storage_errors::provider::ProviderResult;

/// The trait for fetching provider statistics.
#[auto_impl::auto_impl(&, Arc)]
pub trait StatsReader: Send + Sync {
    /// Fetch the number of entries in the corresponding [Table]. Depending on the provider, it may
    /// route to different data sources other than [Table].
    fn count_entries<T: Table>(&self) -> ProviderResult<usize>;
}
