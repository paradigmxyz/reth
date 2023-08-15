use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::{Database, DatabaseGAT},
    redb::tx::{ReadTx, WriteTx},
    table::{Compress, Decode, Decompress, DupSort, Encode, Table, TableImporter},
    transaction::{DbTx, DbTxGAT, DbTxMut, DbTxMutGAT},
};
use bytes::Bytes;
use futures::TryStreamExt;
use redb::{
    AccessGuard, Range, ReadableMultimapTable, ReadableTable, StorageError, TableError,
    TransactionError,
};
use reth_interfaces::db::DatabaseError;
use std::{borrow::Borrow, ops::RangeBounds};

pub mod cursor;
pub mod tx;

// todo: can we simplify this gat shenanigans if we don't use mdbx anymore? :thinking:
impl<'a> DatabaseGAT<'a> for redb::Database {
    type TX = ReadTx<'a>;
    type TXMut = WriteTx<'a>;
}

impl Database for redb::Database {
    fn tx(&self) -> Result<<Self as DatabaseGAT<'_>>::TX, DatabaseError> {
        Ok(ReadTx(self.begin_read().map_err(map_transaction_error)?))
    }

    fn tx_mut(&self) -> Result<<Self as DatabaseGAT<'_>>::TXMut, DatabaseError> {
        Ok(WriteTx(self.begin_write().map_err(map_transaction_error)?))
    }
}

fn decode_redb_item<T: Table>(
    item: Option<(AccessGuard<'_, Data>, AccessGuard<'_, Data>)>,
) -> Result<Option<(<T as Table>::Key, <T as Table>::Value)>, DatabaseError> {
    if item.is_none() {
        return Ok(None)
    }
    let (k, v) = item.unwrap();

    Ok(Some((
        <<T as Table>::Key as Decode>::decode(k.value().0.as_ref())?,
        <<T as Table>::Value as Decompress>::decompress(v.value().0.as_ref())?,
    )))
}

fn map_storage_error(err: StorageError) -> DatabaseError {
    match err {
        StorageError::Corrupted(_) => todo!(),
        StorageError::ValueTooLarge(_) => todo!(),
        StorageError::Io(_) => todo!(),
        StorageError::LockPoisoned(_) => todo!(),
        _ => unreachable!(),
    }
}

fn map_transaction_error(err: TransactionError) -> DatabaseError {
    match err {
        TransactionError::Storage(err) => map_storage_error(err),
        _ => unreachable!(),
    }
}

fn map_table_error(err: TableError) -> DatabaseError {
    match err {
        TableError::Storage(err) => map_storage_error(err),
        TableError::TableIsMultimap(_) => todo!(),
        TableError::TableAlreadyOpen(_, _) => todo!(),
        TableError::TableDoesNotExist(_) => todo!(),
        TableError::TableIsNotMultimap(_) => todo!(),
        TableError::TableTypeMismatch { .. } => todo!(),
        TableError::TypeDefinitionChanged { .. } => todo!(),
        _ => unreachable!(),
    }
}

// todo describe why this is needed (redbkey/redbvalue)
#[derive(Debug)]
pub struct Data(Bytes);

impl<T> From<T> for Data
where
    T: AsRef<[u8]>,
{
    fn from(value: T) -> Self {
        Self(Bytes::copy_from_slice(value.as_ref()))
    }
}

impl redb::RedbValue for Data {
    type SelfType<'a> = Data;

    type AsBytes<'a> = Bytes;

    fn fixed_width() -> Option<usize> {
        None
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        Self(Bytes::from(data.to_vec()))
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'a,
        Self: 'b,
    {
        value.0.clone()
    }

    fn type_name() -> redb::TypeName {
        redb::TypeName::new("Bytes")
    }
}

impl redb::RedbKey for Data {
    fn compare(data1: &[u8], data2: &[u8]) -> std::cmp::Ordering {
        data1.cmp(data2)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cursor_movement() {}

    #[test]
    fn multimaps() {}

    #[test]
    fn walkers() {}
}
