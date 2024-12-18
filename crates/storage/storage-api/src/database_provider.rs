use reth_db_api::{
    common::KeyValue,
    cursor::DbCursorRO,
    database::Database,
    table::Table,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_prune_types::PruneModes;
use reth_storage_errors::provider::ProviderResult;
use std::ops::{Bound, RangeBounds};

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

    /// Return full table as Vec
    fn table<T: Table>(&self) -> Result<Vec<KeyValue<T>>, DatabaseError>
    where
        T::Key: Default + Ord,
    {
        self.tx_ref()
            .cursor_read::<T>()?
            .walk(Some(T::Key::default()))?
            .collect::<Result<Vec<_>, DatabaseError>>()
    }

    /// Return a list of entries from the table, based on the given range.
    #[inline]
    fn get<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<Vec<KeyValue<T>>, DatabaseError> {
        self.tx_ref().cursor_read::<T>()?.walk_range(range)?.collect::<Result<Vec<_>, _>>()
    }

    /// Iterates over read only values in the given table and collects them into a vector.
    ///
    /// Early-returns if the range is empty, without opening a cursor transaction.
    fn cursor_read_collect<T: Table<Key = u64>>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> ProviderResult<Vec<T::Value>> {
        let capacity = match range_size_hint(&range) {
            Some(0) | None => return Ok(Vec::new()),
            Some(capacity) => capacity,
        };
        let mut cursor = self.tx_ref().cursor_read::<T>()?;
        self.cursor_collect_with_capacity(&mut cursor, range, capacity)
    }

    /// Iterates over read only values in the given table and collects them into a vector.
    fn cursor_collect<T: Table<Key = u64>>(
        &self,
        cursor: &mut impl DbCursorRO<T>,
        range: impl RangeBounds<T::Key>,
    ) -> ProviderResult<Vec<T::Value>> {
        let capacity = range_size_hint(&range).unwrap_or(0);
        self.cursor_collect_with_capacity(cursor, range, capacity)
    }

    /// Iterates over read only values in the given table and collects them into a vector with
    /// capacity.
    fn cursor_collect_with_capacity<T: Table<Key = u64>>(
        &self,
        cursor: &mut impl DbCursorRO<T>,
        range: impl RangeBounds<T::Key>,
        capacity: usize,
    ) -> ProviderResult<Vec<T::Value>> {
        let mut items = Vec::with_capacity(capacity);
        for entry in cursor.walk_range(range)? {
            items.push(entry?.1);
        }
        Ok(items)
    }

    /// Remove list of entries from the table. Returns the number of entries removed.
    #[inline]
    fn remove<T: Table>(&self, range: impl RangeBounds<T::Key>) -> Result<usize, DatabaseError>
    where
        Self::Tx: DbTxMut,
    {
        let mut entries = 0;
        let mut cursor_write = self.tx_ref().cursor_write::<T>()?;
        let mut walker = cursor_write.walk_range(range)?;
        while walker.next().transpose()?.is_some() {
            walker.delete_current()?;
            entries += 1;
        }
        Ok(entries)
    }

    /// Return a list of entries from the table, and remove them, based on the given range.
    #[inline]
    fn take<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<Vec<KeyValue<T>>, DatabaseError>
    where
        Self::Tx: DbTxMut,
    {
        let mut cursor_write = self.tx_ref().cursor_write::<T>()?;
        let mut walker = cursor_write.walk_range(range)?;
        let mut items = Vec::new();
        while let Some(i) = walker.next().transpose()? {
            walker.delete_current()?;
            items.push(i)
        }
        Ok(items)
    }
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

fn range_size_hint(range: &impl RangeBounds<u64>) -> Option<usize> {
    let start = match range.start_bound().cloned() {
        Bound::Included(start) => start,
        Bound::Excluded(start) => start.checked_add(1)?,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound().cloned() {
        Bound::Included(end) => end.saturating_add(1),
        Bound::Excluded(end) => end,
        Bound::Unbounded => return None,
    };
    end.checked_sub(start).map(|x| x as _)
}
