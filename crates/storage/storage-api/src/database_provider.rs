use reth_db_api::{database::Database, transaction::DbTx};
use reth_storage_errors::provider::ProviderResult;

/// Database provider.
pub trait DBProvider {
    /// Underlying database transaction held by the provider.
    type Tx: DbTx;

    /// Returns a reference to the underlying transaction.
    fn tx_ref(&self) -> &Self::Tx;
}

/// Database provider factory.
pub trait DatabaseProviderFactory {
    /// Database this factory produces providers for.
    type DB: Database;

    /// Provider type returned by the factory.
    type Provider: DBProvider<Tx = <Self::DB as Database>::TX>;

    /// Create new read-only database provider.
    fn database_provider_ro(&self) -> ProviderResult<Self::Provider>;
}
