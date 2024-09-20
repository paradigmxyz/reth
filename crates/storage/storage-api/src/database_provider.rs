use reth_db_api::{database::Database, transaction::DbTx};
use reth_prune_types::PruneModes;
use reth_storage_errors::provider::ProviderResult;

/// Database provider.
pub trait DBProvider: Send + Sync + Sized + 'static {
    /// Underlying database transaction held by the provider.
    type Tx: DbTx;

    /// Returns a reference to the underlying transaction.
    fn tx_ref(&self) -> &Self::Tx;

    /// Returns a mutable reference to the underlying transaction.
    fn tx_mut(&mut self) -> &mut Self::Tx;

    /// Consumes the provider and returns the underlying transaction.
    fn into_tx(self) -> Self::Tx;

    /// Disables long-lived read transaction safety guarantees for leaks prevention and
    /// observability improvements.
    ///
    /// CAUTION: In most of the cases, you want the safety guarantees for long read transactions
    /// enabled. Use this only if you're sure that no write transaction is open in parallel, meaning
    /// that Reth as a node is offline and not progressing.
    fn disable_long_read_transaction_safety(mut self) -> Self {
        self.tx_mut().disable_long_read_transaction_safety();
        self
    }

    /// Commit database transaction
    fn commit(self) -> ProviderResult<bool> {
        Ok(self.into_tx().commit()?)
    }

    /// Returns a reference to prune modes.
    fn prune_modes_ref(&self) -> &PruneModes;
}

/// Database provider factory.
#[auto_impl::auto_impl(&, Arc)]
pub trait DatabaseProviderFactory: Send + Sync {
    /// Database this factory produces providers for.
    type DB: Database;

    /// Provider type returned by the factory.
    type Provider: DBProvider<Tx = <Self::DB as Database>::TX>;

    /// Read-write provider type returned by the factory.
    type ProviderRW: DBProvider<Tx = <Self::DB as Database>::TXMut>;

    /// Create new read-only database provider.
    fn database_provider_ro(&self) -> ProviderResult<Self::Provider>;

    /// Create new read-write database provider.
    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW>;
}
