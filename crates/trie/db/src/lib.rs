//! The [database] integration with Merkle-Patricia [trie].
//!
//! [database]: reth_db
//! [trie]: reth_trie

use reth_db_api::transaction::DbTx;

pub mod cursors;
pub mod prefix_set;
pub mod proof;
pub mod state;
pub mod trie;
pub mod walker;

/// New-type that wraps an immutable reference to a transaction.
#[derive(Debug)]
pub struct TxRefWrapper<'a, T>(&'a T);

impl<'a, T> From<&'a T> for TxRefWrapper<'a, T> {
    fn from(value: &'a T) -> Self {
        Self(value)
    }
}

impl<'a, TX: DbTx> Clone for TxRefWrapper<'a, TX> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, TX: DbTx> Copy for TxRefWrapper<'a, TX> {}

/// New-type that wraps a database cursor.
#[derive(Debug, Clone)]
pub struct DbCursorWrapper<C>(C);

impl<C> From<C> for DbCursorWrapper<C> {
    fn from(value: C) -> Self {
        Self(value)
    }
}

// re-export for convenience
pub use reth_trie_common::*;
