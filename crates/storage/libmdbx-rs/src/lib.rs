//! Idiomatic and safe Rust bindings for [libmdbx].
//!
//! # Overview
//!
//! [libmdbx] is a high-performance embedded key-value database based on
//! LMDB, with additional features like nested transactions, automatic
//! compaction, and improved durability options.
//!
//! This crate provides a safe, idiomatic Rust interface for:
//! - Creating and managing memory-mapped database environments
//! - Performing transactional read and write operations
//! - Iterating over key-value pairs with cursors
//! - Custom serialization via the [`TableObject`] trait
//!
//! # Quick Start
//!
//! Databases are stored in a directory on disk. The following example
//! demonstrates creating an environment, writing a key-value pair, and
//! reading it back.
//!
//! ```no_run
//! use reth_libmdbx::{DatabaseFlags, Environment, Geometry, MdbxResult, WriteFlags};
//! use std::path::Path;
//!
//! fn main() -> MdbxResult<()> {
//!     // Open an environment (creates directory if needed)
//!     let env = Environment::builder()
//!         .set_geometry(Geometry {
//!             size: Some(0..(1024 * 1024 * 1024)), // up to 1GB
//!             ..Default::default()
//!         })
//!         .open(Path::new("/tmp/my_database"))?;
//!
//!     // Write data in a read-write transaction
//!     let txn = env.begin_rw_txn()?;
//!     let db = txn.create_db(None, DatabaseFlags::empty())?;
//!     txn.put(db, b"hello", b"world", WriteFlags::empty())?;
//!     txn.commit()?;
//!
//!     // Read data in a read-only transaction
//!     let txn = env.begin_ro_txn()?;
//!     let db = txn.open_db(None)?;
//!     let value: Option<Vec<u8>> = txn.get(db.dbi(), b"hello").expect("read failed");
//!     assert_eq!(value.as_deref(), Some(b"world".as_slice()));
//!
//!     Ok(())
//! }
//! ```
//!
//! # Key Concepts
//!
//! - [`Environment`] - A directory containing one or more databases. Created via
//!   [`Environment::builder()`].
//! - [`TxSync`] and [`TxUnsync`] - Transactions for performing database operations.
//!     - Synchronized transactions (`TxSync`) can be shared between threads.
//!     - Unsynchronized transactions (`TxUnsync`) offer better performance for single-threaded use
//!       cases.
//! - [`RO`] and [`RW`] - Marker types indicating read-only (`RO`) or read-write (`RW`)
//!   transactions.
//! - [`Database`] - A named or unnamed key-value store within an environment.
//!   - Opened with [`TxSync::open_db()`] or [`TxUnsync::open_db()`].
//!   - Created with [`TxSync::create_db()`] or [`TxUnsync::create_db()`].
//! - [`Cursor`]: Enables iteration and positioned access within a database. Created via
//!   [`TxSync::cursor()`] or [`TxUnsync::cursor()`].
//!
//! # Feature Flags
//!
//! - `return-borrowed`: When enabled, iterators return borrowed data (`Cow::Borrowed`) whenever
//!   possible, avoiding allocations. This is faster but the data may change if the transaction
//!   modifies it later, which could trigger undefined behavior. When disabled (default), dirty
//!   pages in write transactions trigger copies for safety.
//! - `read-tx-timeouts`: Enables automatic timeout handling for read transactions that block
//!   writers. Useful for detecting stuck readers.
//!
//! # Custom Zero-copy Deserialization with [`TableObject`]
//!
//! Implement [`TableObject`] to decode custom types directly from the
//! database:
//!
//! ```
//! # use std::borrow::Cow;
//! use reth_libmdbx::{MdbxError, ReadResult, TableObject};
//!
//! struct MyKey([u8; 32]);
//!
//! impl TableObject<'_> for MyKey {
//!     fn decode_borrow(data: Cow<'_, [u8]>) -> ReadResult<Self> {
//!         let arr: [u8; 32] =
//!             data.as_ref().try_into().map_err(|_| MdbxError::DecodeErrorLenDiff)?;
//!         Ok(Self(arr))
//!     }
//! }
//! ```
//!
//! See the [`TableObject`] docs for more examples.
//!
//! # Debug assertions
//!
//! When compiled with debug assertions enabled (the default for
//! `cargo build`), this crate performs additional runtime checks to
//! catch common mistakes.
//!
//! 1. Key sizes are checked against the database's configured `pagesize` and `DatabaseFlags` (e.g.
//!    `INTEGERKEY`).
//! 2. Value sizes are checked against the database's configured `pagesize` and `DatabaseFlags`
//!    (e.g. `INTEGERDUP`).
//! 3. For `append` operations, it checks that the key being appended is greater than the current
//!    last key using lexicographic comparison. This check is skipped for `REVERSE_KEY` and
//!    `REVERSE_DUP` databases since they use different comparison semantics (comparing bytes from
//!    end to beginning).
//!
//! # Provenance
//!
//! This crate incorporates code from [signet-libmdbx] (MIT OR Apache-2.0),
//! which was forked from an earlier version of this crate (reth-libmdbx).
//! The original MDBX bindings were forked from an Apache-licensed version
//! of the `libmdbx-rs` crate. Original LMDB bindings from [lmdb-rs].
//!
//! [libmdbx]: https://github.com/erthink/libmdbx
//! [signet-libmdbx]: https://github.com/init4tech/mdbx
//! [lmdb-rs]: https://github.com/mozilla/lmdb-rs
//!
//! # Imports
//!
//! For most use cases, import from the crate root:
//! ```rust,ignore
//! use reth_libmdbx::{Environment, DatabaseFlags, WriteFlags, Geometry, MdbxResult};
//! ```
//!
//! Transaction and cursor types are returned from `Environment` and transaction
//! methods - you rarely need to import them directly.
//!
//! For advanced usage, import from submodules:
//! - [`tx`] - Transaction type aliases (`RoTxSync`, `RwTxUnsync`, etc.) and cursor type aliases
//! - [`tx::iter`] - Iterator types for cursor iteration
//! - [`sys`] - Environment internals (`EnvironmentKind`, `PageSize`, etc.)

#![warn(
    missing_copy_implementations,
    missing_debug_implementations,
    missing_docs,
    unreachable_pub,
    clippy::missing_const_for_fn,
    rustdoc::all
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![allow(
    clippy::borrow_as_ptr,
    clippy::doc_markdown,
    clippy::use_self,
    clippy::explicit_iter_loop,
    clippy::needless_pass_by_ref_mut,
    clippy::match_same_arms,
    clippy::if_then_some_else_none
)]

pub extern crate reth_mdbx_sys as ffi;

mod codec;
pub use codec::{ObjectLength, TableObject, TableObjectOwned};

#[cfg(feature = "read-tx-timeouts")]
pub use crate::sys::read_transactions::MaxReadTransactionDuration;

mod error;
pub use error::{MdbxError, MdbxResult, ReadError, ReadResult};

mod flags;
pub use flags::{DatabaseFlags, EnvironmentFlags, Mode, SyncMode, WriteFlags};

pub mod sys;
pub use sys::{
    Environment, EnvironmentBuilder, EnvironmentKind, Geometry, HandleSlowReadersCallback,
    HandleSlowReadersReturnCode, Info, PageSize, Stat,
};

pub mod tx;
pub use tx::{CommitLatency, Cursor, Database, TransactionKind, TxSync, TxUnsync, RO, RW};

/// Type alias for backward compatibility.
pub type Transaction<K> = TxSync<K>;

/// Type alias for backward compatibility.
pub type Error = MdbxError;

/// Type alias for backward compatibility.
pub type Result<T> = MdbxResult<T>;

#[cfg(test)]
mod test {
    use super::*;
    use byteorder::{ByteOrder, LittleEndian};
    use tempfile::tempdir;

    /// Regression test for <https://github.com/danburkert/lmdb-rs/issues/21>.
    /// This test reliably segfaults when run against lmdb compiled with opt
    /// level -O3 and newer GCC compilers.
    #[test]
    fn issue_21_regression() {
        const HEIGHT_KEY: [u8; 1] = [0];

        let dir = tempdir().unwrap();

        let env = {
            let mut builder = Environment::builder();
            builder.set_max_dbs(2);
            builder
                .set_geometry(Geometry { size: Some(1_000_000..1_000_000), ..Default::default() });
            builder.open(dir.path()).expect("open mdbx env")
        };

        for height in 0..1000 {
            let mut value = [0u8; 8];
            LittleEndian::write_u64(&mut value, height);
            let tx = env.begin_rw_txn().expect("begin_rw_txn");
            let index = tx.create_db(None, DatabaseFlags::DUP_SORT).expect("open index db");
            tx.put(index, HEIGHT_KEY, value, WriteFlags::empty()).expect("tx.put");
            tx.commit().expect("tx.commit");
        }
    }
}
