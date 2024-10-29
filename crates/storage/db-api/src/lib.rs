//! reth's database abstraction layer.
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
//! [`Database`]: crate::database::Database
//! [`DbTx`]: crate::transaction::DbTx
//! [`DbTxMut`]: crate::transaction::DbTxMut
//! [`DbCursorRO`]: crate::cursor::DbCursorRO
//! [`DbCursorRW`]: crate::cursor::DbCursorRW
//! [`Walker`]: crate::cursor::Walker
//! [`RangeWalker`]: crate::cursor::RangeWalker
//! [`ReverseWalker`]: crate::cursor::ReverseWalker
//! [`DbDupCursorRO`]: crate::cursor::DbDupCursorRO
//! [`Encode`]: crate::table::Encode
//! [`Decode`]: crate::table::Decode
//! [`Compress`]: crate::table::Compress
//! [`Decompress`]: crate::table::Decompress
//! [`Table`]: crate::table::Table

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Common types used throughout the abstraction.
pub mod common;
/// Cursor database traits.
pub mod cursor;
/// Database traits.
pub mod database;
/// Database metrics trait extensions.
pub mod database_metrics;
pub mod mock;
/// Table traits
pub mod table;
/// Transaction database traits.
pub mod transaction;
/// Re-exports
pub use reth_storage_errors::db::{DatabaseError, DatabaseWriteOperation};

pub mod models;
mod scale;

mod utils;

pub use database::Database;

mod unwind;
pub use unwind::DbTxUnwindExt;
