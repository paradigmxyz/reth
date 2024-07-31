use std::ops::RangeInclusive;

use reth_primitives::{Account, Address, BlockNumber, B256, U256};
use reth_storage_errors::db::DatabaseError;

/// A trait for creating iterators walking over a range of block numbers.
#[auto_impl::auto_impl(&mut, Box)]
pub trait RangeWalker<V>: Send + Sync {
    /// Creates an iterator that walks over a range of block numbers.
    fn walk_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<impl Iterator<Item = Result<V, DatabaseError>>, DatabaseError>;
}

/// Factory for creating changeset range walkers.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieRangeWalkerFactory: Send + Sync {
    /// Cursor that walks over account changesets.
    type AccountCursor: RangeWalker<(Address, Option<Account>)>;

    /// Cursor that walks over storage changesets.
    type StorageCursor: RangeWalker<(Address, B256, U256)>;

    /// Creates account changesets cursor.
    fn account_changesets(&self) -> Result<Self::AccountCursor, DatabaseError>;

    /// Creates storage changesets cursor.
    fn storage_changesets(&self) -> Result<Self::StorageCursor, DatabaseError>;
}
