use crate::{
    redb::{
        cursor::{CursorRO, CursorRW, DupCursorRO, DupCursorRW},
        map_storage_error, map_table_error, Data,
    },
    table::{Compress, Decompress, DupSort, Encode, Table, TableImporter},
    transaction::{DbTx, DbTxGAT, DbTxMut, DbTxMutGAT},
};
use futures::TryStreamExt;
use redb::{
    MultimapTableDefinition, ReadableMultimapTable, ReadableTable, TableDefinition,
    WriteTransaction,
};
use reth_interfaces::db::DatabaseError;

// todo: verify that the tx is *aborted* on drop (as is the case w mdbx)
pub struct WriteTx<'a>(pub(crate) WriteTransaction<'a>);

impl<'a> std::fmt::Debug for WriteTx<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteTx").finish()
    }
}

impl<'a> DbTxGAT<'a> for WriteTx<'_> {
    type Cursor<T: Table> = CursorRO<redb::Table<'a, 'a, Data, Data>, T>;
    type DupCursor<T: DupSort> = DupCursorRO<'a, T>;
}

impl<'a> DbTxMutGAT<'a> for WriteTx<'_> {
    type CursorMut<T: Table> = CursorRW<'a, T>;
    type DupCursorMut<T: DupSort> = DupCursorRW<'a, T>;
}

impl<'a> TableImporter<'a> for WriteTx<'a> {}

impl<'tx> DbTx<'tx> for WriteTx<'tx> {
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        // NOTE: This ONLY supports normal tables, not multimaps. Ideally this would be enforced by
        // the compiler, but separating `DupSort` from `Table` at this point is super hard (I've
        // tried)
        let table: redb::Table<'_, '_, &[u8], &[u8]> =
            self.0.open_table(TableDefinition::new(T::NAME)).map_err(map_table_error)?;
        let value =
            if let Some(value) = table.get(key.encode().as_ref()).map_err(map_storage_error)? {
                Some(<<T as Table>::Value>::decompress(value.value())?)
            } else {
                None
            };

        Ok(value)
    }

    fn get_dup<T: DupSort>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        let table: redb::MultimapTable<'_, '_, &[u8], &[u8]> = self
            .0
            .open_multimap_table(MultimapTableDefinition::new(T::NAME))
            .map_err(map_table_error)?;
        let value = if let Some(value) = table
            .get(key.encode().as_ref())
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?
        {
            Some(<<T as Table>::Value>::decompress(value.value())?)
        } else {
            None
        };

        Ok(value)
    }

    fn drop(self) {
        // todo: this can fail - why?
        self.0.abort().unwrap();
    }

    fn cursor_read<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, DatabaseError> {
        todo!()
    }

    fn cursor_dup_read<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, DatabaseError> {
        todo!()
    }

    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        Ok(self.0.open_table(TableDefinition::<&[u8], &[u8]>::new(T::NAME)).unwrap().len().unwrap()
            as usize)
    }

    fn entries_dup<T: Table>(&self) -> Result<usize, DatabaseError> {
        // todo errs
        Ok(self
            .0
            .open_multimap_table(MultimapTableDefinition::<&[u8], &[u8]>::new(T::NAME))
            .unwrap()
            .len()
            .unwrap() as usize)
    }
}

impl DbTxMut<'_> for WriteTx<'_> {
    fn commit(self) -> Result<bool, DatabaseError> {
        self.0.commit().map_err(|err| match err {
            redb::CommitError::Storage(err) => map_storage_error(err),
            _ => DatabaseError::Commit(0),
        })?;

        // todo: what does the bool mean
        Ok(true)
    }

    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        // TODO: needs a dup equivalent
        self.0
            .open_table(TableDefinition::<Data, Data>::new(T::NAME))
            .map_err(map_table_error)?
            .insert(Data::from(key.encode()), Data::from(value.compress()))
            .map_err(map_storage_error)?;

        Ok(())
    }

    fn delete<T: Table>(
        &self,
        key: T::Key,
        value: Option<T::Value>,
    ) -> Result<bool, DatabaseError> {
        // TODO: remove `value` from this and put it in a `delete_dup` instead.
        Ok(self
            .0
            .open_table(TableDefinition::<Data, Data>::new(T::NAME))
            .map_err(map_table_error)?
            .remove(Data::from(key.encode()))
            .map_err(map_storage_error)?
            .is_some())
    }

    fn clear<T: Table>(&self) -> Result<(), DatabaseError> {
        // TODO: there is no other way (seemingly) to clear the table
        // TODO: there needs to be a `clear_dup` too..
        while let Some(_) = self
            .0
            .open_table(TableDefinition::<Data, Data>::new(T::NAME))
            .map_err(map_table_error)?
            .drain::<Data>(..)
            .map_err(map_storage_error)?
            .next()
        {}

        Ok(())
    }

    fn cursor_write<T: Table>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, DatabaseError> {
        todo!()
    }

    fn cursor_dup_write<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, DatabaseError> {
        todo!()
    }
}

///

pub struct ReadTx<'a>(pub(crate) redb::ReadTransaction<'a>);

impl<'a> std::fmt::Debug for ReadTx<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadTx").finish()
    }
}

impl<'a> DbTxGAT<'a> for ReadTx<'_> {
    type Cursor<T: Table> = CursorRO<redb::ReadOnlyTable<'a, Data, Data>, T>;
    type DupCursor<T: DupSort> = DupCursorRO<'a, T>;
}

impl<'a> DbTx<'a> for ReadTx<'a> {
    fn get<T: Table>(&self, _key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        todo!()
    }

    fn get_dup<T: DupSort>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        todo!()
    }

    fn drop(self) {
        // noop
    }

    fn cursor_read<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, DatabaseError> {
        todo!()
    }

    fn cursor_dup_read<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, DatabaseError> {
        todo!()
    }

    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        Ok(self.0.open_table(TableDefinition::<&[u8], &[u8]>::new(T::NAME)).unwrap().len().unwrap()
            as usize)
    }

    fn entries_dup<T: Table>(&self) -> Result<usize, DatabaseError> {
        // todo errs
        Ok(self
            .0
            .open_multimap_table(MultimapTableDefinition::<&[u8], &[u8]>::new(T::NAME))
            .unwrap()
            .len()
            .unwrap() as usize)
    }
}
