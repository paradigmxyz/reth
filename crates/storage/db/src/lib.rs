//! reth's database abstraction layer with concrete implementations.
//!
//! The database abstraction assumes that the underlying store is a KV store subdivided into tables.
//!
//! One or more changes are tied to a transaction that is atomically committed to the data store at
//! the same time. Strong consistency in what data is written and when is important for reth, so it
//! is not possible to write data to the database outside of using a transaction.
//!
//! Good starting points for this crate are:
//!
//! - [`Database`] for the main database abstraction
//! - [`DbTx`] (RO) and [`DbTxMut`] (RW) for the transaction abstractions.
//! - [`DbCursorRO`] (RO) and [`DbCursorRW`] (RW) for the cursor abstractions (see below).
//!
//! # Cursors and Walkers
//!
//! The abstraction also defines a couple of helpful abstractions for iterating and writing data:
//!
//! - **Cursors** ([`DbCursorRO`] / [`DbCursorRW`]) for iterating data in a table. Cursors are
//!   assumed to resolve data in a sorted manner when iterating from start to finish, and it is safe
//!   to assume that they are efficient at doing so.
//! - **Walkers** ([`Walker`] / [`RangeWalker`] / [`ReverseWalker`]) use cursors to walk the entries
//!   in a table, either fully from a specific point, or over a range.
//!
//! Dup tables (see below) also have corresponding cursors and walkers (e.g. [`DbDupCursorRO`]).
//! These **should** be preferred when working with dup tables, as they provide additional methods
//! that are optimized for dup tables.
//!
//! # Tables
//!
//! reth has two types of tables: simple KV stores (one key, one value) and dup tables (one key,
//! many values). Dup tables can be efficient for certain types of data.
//!
//! Keys are de/serialized using the [`Encode`] and [`Decode`] traits, and values are de/serialized
//! ("compressed") using the [`Compress`] and [`Decompress`] traits.
//!
//! Tables implement the [`Table`] trait.
//!
//! # Overview
//!
//! An overview of the current data model of reth can be found in the [`tables`] module.
//!
//! [`Database`]: crate::abstraction::database::Database
//! [`DbTx`]: crate::abstraction::transaction::DbTx
//! [`DbTxMut`]: crate::abstraction::transaction::DbTxMut
//! [`DbCursorRO`]: crate::abstraction::cursor::DbCursorRO
//! [`DbCursorRW`]: crate::abstraction::cursor::DbCursorRW
//! [`Walker`]: crate::abstraction::cursor::Walker
//! [`RangeWalker`]: crate::abstraction::cursor::RangeWalker
//! [`ReverseWalker`]: crate::abstraction::cursor::ReverseWalker
//! [`DbDupCursorRO`]: crate::abstraction::cursor::DbDupCursorRO
//! [`Encode`]: crate::abstraction::table::Encode
//! [`Decode`]: crate::abstraction::table::Decode
//! [`Compress`]: crate::abstraction::table::Compress
//! [`Decompress`]: crate::abstraction::table::Decompress
//! [`Table`]: crate::abstraction::table::Table

#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

/// Traits defining the database abstractions, such as cursors and transactions.
pub mod abstraction;

mod implementation;
pub mod tables;
mod utils;

#[cfg(feature = "mdbx")]
/// Bindings for [MDBX](https://libmdbx.dqdkfa.ru/).
pub mod mdbx {
    pub use crate::implementation::mdbx::*;
    pub use reth_libmdbx::*;
}

pub use abstraction::*;
pub use reth_interfaces::db::DatabaseError;
pub use tables::*;
