use crate::{
    table::{Table, TableImporter},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use std::{fmt::Debug, sync::Arc};

/// Main Database trait that can open read-only and read-write transactions.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for consumption.
pub trait Database: Sized + Send + Sync {
    /// A read-only database transaction
    type TX: DbTx + Send + Sync + Debug + 'static;
    /// A read-write database transaction
    type TXMut: DbTxMut + DbTx + TableImporter + Send + Sync + Debug + 'static;
    /// Database-specific options for opening an existing database, and initializing a new database.
    type Opts: Debug;

    /// Open the database in read-write mode with the given options, initializing it with the given
    /// tables if it does not already exist.
    fn open(opts: Self::Opts) -> eyre::Result<Self>;

    /// Open the database in readonly mode with the given options.
    fn open_ro(opts: Self::Opts) -> eyre::Result<Self>;

    /// Create read only transaction.
    #[track_caller]
    fn tx(&self) -> Result<Self::TX, DatabaseError>;

    /// Create read write transaction only possible if database is open with write access.
    #[track_caller]
    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError>;

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    fn view<T, F>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&Self::TX) -> T,
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
        F: FnOnce(&Self::TXMut) -> T,
    {
        let tx = self.tx_mut()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}

impl<DB: Database> Database for Arc<DB> {
    type TX = <DB as Database>::TX;
    type TXMut = <DB as Database>::TXMut;
    type Opts = <DB as Database>::Opts;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }

    fn open(opts: Self::Opts) -> eyre::Result<Self> {
        <DB as Database>::open(opts).map(Arc::new)
    }

    fn init(opts: Self::Opts) -> eyre::Result<Self> {
        <DB as Database>::init(opts).map(Arc::new)
    }

    fn open_ro(opts: Self::Opts) -> eyre::Result<Self> {
        <DB as Database>::open_ro(opts).map(Arc::new)
    }
}

impl<DB: Database> Database for &DB {
    type TX = <DB as Database>::TX;
    type TXMut = <DB as Database>::TXMut;
    type Opts = <DB as Database>::Opts;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }

    fn open(_: Self::Opts) -> eyre::Result<Self> {
        unimplemented!()
    }

    fn init(_: Self::Opts) -> eyre::Result<Self> {
        unimplemented!()
    }

    fn open_ro(_: Self::Opts) -> eyre::Result<Self> {
        unimplemented!()
    }
}
