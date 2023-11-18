use crate::{
    common::Sealed,
    table::TableImporter,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use std::{fmt::Debug, sync::Arc};

/// Trait that provides the different transaction types.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DatabaseGAT: Sealed + Send + Sync {

}

/// Main Database trait that spawns transactions to be executed.
pub trait Database: DatabaseGAT {
    /// RO database transaction
    type TX: DbTx + Send + Sync + Debug;
    /// RW database transaction
    type TXMut: DbTxMut + DbTx + TableImporter + Send + Sync + Debug;

    /// Create read only transaction.
    fn tx(&self) -> Result<<Self as DatabaseGAT>::TX, DatabaseError>;

    /// Create read write transaction only possible if database is open with write access.
    fn tx_mut(&self) -> Result<<Self as DatabaseGAT>::TXMut, DatabaseError>;

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    fn view<T, F>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&<Self as DatabaseGAT>::TX) -> T,
    {
        let tx = self.tx()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }

    /// Takes a function and passes a write-read transaction into it, making sure it's committed in
    /// the end of the execution.
    fn update<T, F>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&<Self as DatabaseGAT>::TXMut) -> T,
    {
        let tx = self.tx_mut()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}

// Generic over Arc
impl<DB: Database> DatabaseGAT for Arc<DB> {
    type TX = <DB as DatabaseGAT>::TX;
    type TXMut = <DB as DatabaseGAT>::TXMut;
}

impl<DB: Database> Database for Arc<DB> {
    fn tx(&self) -> Result<<Self as DatabaseGAT>::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<<Self as DatabaseGAT>::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }
}

// Generic over reference
impl<DB: Database> DatabaseGAT for &DB {
    type TX = <DB as DatabaseGAT>::TX;
    type TXMut = <DB as DatabaseGAT>::TXMut;
}

impl<DB: Database> Database for &DB {
    fn tx(&self) -> Result<<Self as DatabaseGAT>::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<<Self as DatabaseGAT>::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }
}
