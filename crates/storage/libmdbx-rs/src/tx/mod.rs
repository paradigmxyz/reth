//! Transaction management and access.
//!
//! # Core Types (re-exported at crate root)
//!
//! - [`TxSync`] - Thread-safe synchronized transaction
//! - [`TxUnsync`] - Single-threaded unsynchronized transaction
//! - [`Cursor`] - Database cursor for navigating entries
//! - [`Database`] - Handle to an opened database
//! - [`RO`], [`RW`] - Transaction kind markers
//! - [`CommitLatency`] - Commit timing information
//!
//! # Type Aliases
//!
//! Convenience aliases for common transaction/cursor configurations:
//! - [`RoTxSync`], [`RwTxSync`] - Synchronized transactions
//! - [`RoTxUnsync`], [`RwTxUnsync`] - Unsynchronized transactions
//! - [`RoCursorSync`], [`RwCursorSync`] - Cursors for synchronized transactions
//! - [`RoCursorUnsync`], [`RwCursorUnsync`] - Cursors for unsynchronized transactions
//!
//! # Advanced: Writing Generic Code
//!
//! For users writing generic code over cursors or transactions, the
//! [`TxPtrAccess`] trait is available. This trait abstracts over the different
//! ways transaction pointers are stored and accessed.

mod access;
pub(crate) use access::PtrSync;
pub use access::{PtrSyncInner, RoGuard, RwUnsync, TxPtrAccess};
mod assertions;

mod cache;
pub(crate) use cache::{CachedDb, SharedCache};

mod cursor;
pub use cursor::{Cursor, RoCursorSync, RoCursorUnsync, RwCursorSync, RwCursorUnsync};

mod database;
pub use database::Database;

pub mod iter;
pub use iter::{RoIterSync, RoIterUnsync, RwIterSync, RwIterUnsync};

mod kind;
pub use kind::{TransactionKind, RO, RW};

/// Raw operations on transactions.
pub mod ops;

mod sync;
#[allow(unused_imports)] // this is used in some features
pub use sync::{CommitLatency, TxSync};

pub mod unsync;
pub use unsync::TxUnsync;

/// A synchronized read-only transaction.
pub type RoTxSync = TxSync<RO>;

/// A synchronized read-write transaction.
pub type RwTxSync = TxSync<RW>;

/// An unsynchronized read-only transaction.
pub type RoTxUnsync = TxUnsync<RO>;

/// An unsynchronized read-write transaction.
pub type RwTxUnsync = TxUnsync<RW>;

/// The default maximum duration of a read transaction.
#[cfg(feature = "read-tx-timeouts")]
pub const DEFAULT_MAX_READ_TRANSACTION_DURATION: std::time::Duration =
    std::time::Duration::from_secs(5 * 60);
