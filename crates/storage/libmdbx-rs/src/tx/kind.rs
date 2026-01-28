use crate::tx::{
    access::{RoGuard, RwUnsync},
    TxPtrAccess,
};
use ffi::{MDBX_txn_flags_t, MDBX_TXN_RDONLY, MDBX_TXN_READWRITE};

mod private {
    pub trait Sealed {}
    impl Sealed for super::RO {}
    impl Sealed for super::RW {}
}

/// Marker type for read-only transactions.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RO;

/// Marker type for read-write transactions.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RW;

/// Marker trait for transaction kinds with associated inner type.
///
/// The `Inner` associated type determines how the transaction pointer is
/// stored:
/// - For [`RO`]: Either `RoInner` (Arc/Weak) with feature `read-tx-timeouts`, or raw pointer
///   without it
/// - For [`RW`]: Always raw pointer (direct ownership)
pub trait TransactionKind: private::Sealed + core::fmt::Debug + 'static {
    #[doc(hidden)]
    const OPEN_FLAGS: MDBX_txn_flags_t;

    /// Whether this is a read-only transaction.
    const IS_READ_ONLY: bool;

    /// The inner storage type for the transaction pointer.
    type Inner: TxPtrAccess;
}

impl TransactionKind for RO {
    const OPEN_FLAGS: MDBX_txn_flags_t = MDBX_TXN_RDONLY;
    const IS_READ_ONLY: bool = true;

    // Without timeouts, RO uses direct pointer like RW
    type Inner = RoGuard;
}

impl TransactionKind for RW {
    const OPEN_FLAGS: MDBX_txn_flags_t = MDBX_TXN_READWRITE;
    const IS_READ_ONLY: bool = false;

    type Inner = RwUnsync;
}
