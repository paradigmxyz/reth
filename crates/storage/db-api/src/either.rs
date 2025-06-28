//! Contains `DbTx` and `DbTxMut` traits implementation for `Either`.

use crate::{
    common::{IterPairResult, PairResult, ValueOnlyResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, RangeWalker,
        ReverseWalker, Walker,
    },
    table::{DupSort, Encode, Table},
    transaction::{DbTx, DbTxMut},
};
pub use either::Either;
use reth_storage_errors::db::DatabaseError;
use std::ops::{Bound, RangeBounds};

impl<T, L, R> DbCursorRO<T> for Either<L, R>
where
    T: Table,
    L: DbCursorRO<T>,
    R: DbCursorRO<T>,
{
    fn first(&mut self) -> PairResult<T> {
        match self {
            Self::Left(l) => l.first(),
            Self::Right(r) => r.first(),
        }
    }

    fn seek_exact(&mut self, key: T::Key) -> PairResult<T> {
        match self {
            Self::Left(l) => l.seek_exact(key),
            Self::Right(r) => r.seek_exact(key),
        }
    }

    fn seek(&mut self, key: T::Key) -> PairResult<T> {
        match self {
            Self::Left(l) => l.seek(key),
            Self::Right(r) => r.seek(key),
        }
    }

    fn next(&mut self) -> PairResult<T> {
        match self {
            Self::Left(l) => l.next(),
            Self::Right(r) => r.next(),
        }
    }

    fn prev(&mut self) -> PairResult<T> {
        match self {
            Self::Left(l) => l.prev(),
            Self::Right(r) => r.prev(),
        }
    }

    fn last(&mut self) -> PairResult<T> {
        match self {
            Self::Left(l) => l.last(),
            Self::Right(r) => r.last(),
        }
    }

    fn current(&mut self) -> PairResult<T> {
        match self {
            Self::Left(l) => l.current(),
            Self::Right(r) => r.current(),
        }
    }

    fn walk(&mut self, start_key: Option<T::Key>) -> Result<Walker<'_, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        match self {
            Self::Left(_) | Self::Right(_) => {
                let start: IterPairResult<T> = match start_key {
                    Some(key) => <Self as DbCursorRO<T>>::seek(self, key).transpose(),
                    None => <Self as DbCursorRO<T>>::first(self).transpose(),
                };

                Ok(Walker::new(self, start))
            }
        }
    }

    fn walk_range(
        &mut self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<RangeWalker<'_, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        match self {
            Self::Left(_) | Self::Right(_) => {
                let start_key = match range.start_bound() {
                    Bound::Included(key) | Bound::Excluded(key) => Some((*key).clone()),
                    Bound::Unbounded => None,
                };

                let end_key = match range.end_bound() {
                    Bound::Included(key) | Bound::Excluded(key) => Bound::Included((*key).clone()),
                    Bound::Unbounded => Bound::Unbounded,
                };

                let start: IterPairResult<T> = match start_key {
                    Some(key) => <Self as DbCursorRO<T>>::seek(self, key).transpose(),
                    None => <Self as DbCursorRO<T>>::first(self).transpose(),
                };

                Ok(RangeWalker::new(self, start, end_key))
            }
        }
    }

    fn walk_back(
        &mut self,
        start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'_, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        match self {
            Self::Left(_) | Self::Right(_) => {
                let start: IterPairResult<T> = match start_key {
                    Some(key) => <Self as DbCursorRO<T>>::seek(self, key).transpose(),
                    None => <Self as DbCursorRO<T>>::last(self).transpose(),
                };
                Ok(ReverseWalker::new(self, start))
            }
        }
    }
}

impl<T, L, R> DbDupCursorRO<T> for Either<L, R>
where
    T: DupSort,
    L: DbDupCursorRO<T> + DbCursorRO<T>,
    R: DbDupCursorRO<T> + DbCursorRO<T>,
{
    fn next_dup(&mut self) -> PairResult<T> {
        match self {
            Self::Left(l) => l.next_dup(),
            Self::Right(r) => r.next_dup(),
        }
    }

    fn next_no_dup(&mut self) -> PairResult<T> {
        match self {
            Self::Left(l) => l.next_no_dup(),
            Self::Right(r) => r.next_no_dup(),
        }
    }

    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        match self {
            Self::Left(l) => l.next_dup_val(),
            Self::Right(r) => r.next_dup_val(),
        }
    }

    fn seek_by_key_subkey(&mut self, key: T::Key, subkey: T::SubKey) -> ValueOnlyResult<T> {
        match self {
            Self::Left(l) => l.seek_by_key_subkey(key, subkey),
            Self::Right(r) => r.seek_by_key_subkey(key, subkey),
        }
    }

    fn walk_dup(
        &mut self,
        _key: Option<T::Key>,
        _subkey: Option<T::SubKey>,
    ) -> Result<DupWalker<'_, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        match self {
            Self::Left(_) | Self::Right(_) => Ok(DupWalker { cursor: self, start: None }),
        }
    }
}

impl<T, L, R> DbCursorRW<T> for Either<L, R>
where
    T: Table,
    L: DbCursorRW<T>,
    R: DbCursorRW<T>,
{
    fn upsert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.upsert(key, value),
            Self::Right(r) => r.upsert(key, value),
        }
    }

    fn insert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.insert(key, value),
            Self::Right(r) => r.insert(key, value),
        }
    }

    fn append(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.append(key, value),
            Self::Right(r) => r.append(key, value),
        }
    }

    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.delete_current(),
            Self::Right(r) => r.delete_current(),
        }
    }
}

impl<T, L, R> DbDupCursorRW<T> for Either<L, R>
where
    T: DupSort,
    L: DbDupCursorRW<T>,
    R: DbDupCursorRW<T>,
{
    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.append_dup(key, value),
            Self::Right(r) => r.append_dup(key, value),
        }
    }

    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.delete_current_duplicates(),
            Self::Right(r) => r.delete_current_duplicates(),
        }
    }
}

impl<L, R> DbTx for Either<L, R>
where
    L: DbTx,
    R: DbTx,
{
    type DupCursor<T: DupSort> = Either<L::DupCursor<T>, R::DupCursor<T>>;
    type Cursor<T: Table> = Either<L::Cursor<T>, R::Cursor<T>>;

    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        match self {
            Self::Left(l) => l.get::<T>(key),
            Self::Right(r) => r.get::<T>(key),
        }
    }

    fn get_by_encoded_key<T: Table>(
        &self,
        key: &<T::Key as Encode>::Encoded,
    ) -> Result<Option<T::Value>, DatabaseError> {
        match self {
            Self::Left(l) => l.get_by_encoded_key::<T>(key),
            Self::Right(r) => r.get_by_encoded_key::<T>(key),
        }
    }

    fn commit(self) -> Result<bool, DatabaseError> {
        match self {
            Self::Left(l) => l.commit(),
            Self::Right(r) => r.commit(),
        }
    }

    fn abort(self) {
        match self {
            Self::Left(l) => l.abort(),
            Self::Right(r) => r.abort(),
        }
    }

    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError> {
        match self {
            Self::Left(l) => l.cursor_read().map(Either::Left),
            Self::Right(r) => r.cursor_read().map(Either::Right),
        }
    }

    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError> {
        match self {
            Self::Left(l) => l.cursor_dup_read().map(Either::Left),
            Self::Right(r) => r.cursor_dup_read().map(Either::Right),
        }
    }

    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        match self {
            Self::Left(l) => l.entries::<T>(),
            Self::Right(r) => r.entries::<T>(),
        }
    }

    fn disable_long_read_transaction_safety(&mut self) {
        match self {
            Self::Left(l) => l.disable_long_read_transaction_safety(),
            Self::Right(r) => r.disable_long_read_transaction_safety(),
        }
    }
}

impl<L, R> DbTxMut for Either<L, R>
where
    L: DbTxMut,
    R: DbTxMut,
{
    type DupCursorMut<T: DupSort> = Either<L::DupCursorMut<T>, R::DupCursorMut<T>>;
    type CursorMut<T: Table> = Either<L::CursorMut<T>, R::CursorMut<T>>;

    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.put::<T>(key, value),
            Self::Right(r) => r.put::<T>(key, value),
        }
    }

    fn delete<T: Table>(
        &self,
        key: T::Key,
        value: Option<T::Value>,
    ) -> Result<bool, DatabaseError> {
        match self {
            Self::Left(l) => l.delete::<T>(key, value),
            Self::Right(r) => r.delete::<T>(key, value),
        }
    }

    fn clear<T: Table>(&self) -> Result<(), DatabaseError> {
        match self {
            Self::Left(l) => l.clear::<T>(),
            Self::Right(r) => r.clear::<T>(),
        }
    }

    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError> {
        match self {
            Self::Left(l) => l.cursor_write().map(Either::Left),
            Self::Right(r) => r.cursor_write().map(Either::Right),
        }
    }

    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, DatabaseError> {
        match self {
            Self::Left(l) => l.cursor_dup_write().map(Either::Left),
            Self::Right(r) => r.cursor_dup_write().map(Either::Right),
        }
    }
}
