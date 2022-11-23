pub(crate) mod opt {
    use tokio::sync::mpsc::{error::SendError, Sender};

    /// Get an [Option] with the maximum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn max<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::max(v, b)))
    }

    /// Get an [Option] with the minimum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn min<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::min(v, b)))
    }

    /// The producing side of a [tokio::mpsc] channel that may or may not be set.
    #[derive(Default, Clone)]
    pub(crate) struct MaybeSender<T> {
        inner: Option<Sender<T>>,
    }

    impl<T> MaybeSender<T> {
        /// Create a new [MaybeSender]
        pub(crate) fn new(sender: Option<Sender<T>>) -> Self {
            Self { inner: sender }
        }

        /// Send a value over the channel if an internal sender has been set.
        pub(crate) async fn send(&self, value: T) -> Result<(), SendError<T>> {
            if let Some(rx) = &self.inner {
                rx.send(value).await
            } else {
                Ok(())
            }
        }

        /// Set or unset the internal sender.
        pub(crate) fn set(&mut self, sender: Option<Sender<T>>) {
            self.inner = sender;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn opt_max() {
            assert_eq!(max(None, 5), Some(5));
            assert_eq!(max(Some(1), 5), Some(5));
            assert_eq!(max(Some(10), 5), Some(10));
        }

        #[test]
        fn opt_min() {
            assert_eq!(min(None, 5), Some(5));
            assert_eq!(min(Some(1), 5), Some(1));
            assert_eq!(min(Some(10), 5), Some(5));
        }
    }
}

pub(crate) mod unwind {
    use reth_interfaces::db::{
        models::BlockNumHash, Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbTxMut, Error, Table,
    };
    use reth_primitives::BlockNumber;

    /// Unwind table by some number key
    #[inline]
    pub(crate) fn unwind_table_by_num<DB, T>(
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        num: u64,
    ) -> Result<(), Error>
    where
        DB: Database,
        T: Table<Key = u64>,
    {
        unwind_table::<DB, T, _>(tx, num, |key| key)
    }

    /// Unwind table by composite block number hash key
    #[inline]
    pub(crate) fn unwind_table_by_num_hash<DB, T>(
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        block: BlockNumber,
    ) -> Result<(), Error>
    where
        DB: Database,
        T: Table<Key = BlockNumHash>,
    {
        unwind_table::<DB, T, _>(tx, block, |key| key.number())
    }

    /// Unwind the table to a provided block
    pub(crate) fn unwind_table<DB, T, F>(
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        block: BlockNumber,
        mut selector: F,
    ) -> Result<(), Error>
    where
        DB: Database,
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        let mut cursor = tx.cursor_mut::<T>()?;
        let mut entry = cursor.last()?;
        while let Some((key, _)) = entry {
            if selector(key) <= block {
                break
            }
            cursor.delete_current()?;
            entry = cursor.prev()?;
        }
        Ok(())
    }

    /// Unwind a table forward by a [Walker] on another table
    pub(crate) fn unwind_table_by_walker<DB, T1, T2>(
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        start_at: T1::Key,
    ) -> Result<(), Error>
    where
        DB: Database,
        T1: Table,
        T2: Table<Key = T1::Value>,
    {
        let mut cursor = tx.cursor_mut::<T1>()?;
        let mut walker = cursor.walk(start_at)?;
        while let Some((_, value)) = walker.next().transpose()? {
            tx.delete::<T2>(value, None)?;
        }
        Ok(())
    }
}

pub(crate) mod db {
    use std::{
        fmt::Debug,
        ops::{Deref, DerefMut},
    };

    use reth_interfaces::db::{
        models::{BlockNumHash, NumTransactions},
        tables, DBContainer, Database, DatabaseGAT, DbTx, Error,
    };
    use reth_primitives::{BlockHash, BlockNumber};

    use crate::{DatabaseIntegrityError, StageError};

    /// TODO: doc
    pub struct StageDB<'a, DB: Database>(DBContainer<'a, DB>);

    impl<'a, DB: Database> Debug for StageDB<'a, DB> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("StageDB").finish()
        }
    }

    impl<'a, DB: Database> Deref for StageDB<'a, DB> {
        type Target = <DB as DatabaseGAT<'a>>::TXMut;

        fn deref(&self) -> &Self::Target {
            self.0.get()
        }
    }

    impl<'a, DB: Database> DerefMut for StageDB<'a, DB> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.0.get_mut()
        }
    }

    impl<'a, DB: Database> StageDB<'a, DB> {
        /// Create new instance
        pub(crate) fn new(db: &'a DB) -> Result<Self, Error> {
            Ok(Self(DBContainer::new(db)?))
        }

        pub(crate) fn commit(&mut self) -> Result<bool, Error> {
            self.0.commit()
        }

        /// Query [tables::CanonicalHeaders] table for block hash by block number
        pub(crate) fn get_block_hash(&self, number: BlockNumber) -> Result<BlockHash, StageError> {
            let hash = self
                .get::<tables::CanonicalHeaders>(number)?
                .ok_or(DatabaseIntegrityError::CanonicalHash { number })?;
            Ok(hash)
        }

        /// Query for block hash by block number and return it as [BlockNumHash] key
        pub(crate) fn get_block_numhash(
            &self,
            number: BlockNumber,
        ) -> Result<BlockNumHash, StageError> {
            Ok((number, self.get_block_hash(number)?).into())
        }

        /// Query [tables::CumulativeTxCount] table for total transaction
        /// count block by [BlockNumHash] key
        pub(crate) fn get_tx_count(
            &self,
            key: BlockNumHash,
        ) -> Result<NumTransactions, StageError> {
            let count = self.get::<tables::CumulativeTxCount>(key)?.ok_or(
                DatabaseIntegrityError::CumulativeTxCount {
                    number: key.number(),
                    hash: key.hash(),
                },
            )?;
            Ok(count)
        }
    }
}
