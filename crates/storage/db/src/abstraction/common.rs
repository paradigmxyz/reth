use crate::{abstraction::table::*, DatabaseError};

/// A key-value pair for table `T`.
pub type KeyValue<T> = (<T as Table>::Key, <T as Table>::Value);

/// A fallible key-value pair that may or may not exist.
///
/// The `Result` represents that the operation might fail, while the `Option` represents whether or
/// not the entry exists.
pub type PairResult<T> = Result<Option<KeyValue<T>>, DatabaseError>;

/// A key-value pair coming from an iterator.
///
/// The `Result` represents that the operation might fail, while the `Option` represents whether or
/// not there is another entry.
pub type IterPairResult<T> = Option<Result<KeyValue<T>, DatabaseError>>;

/// A value only result for table `T`.
pub type ValueOnlyResult<T> = Result<Option<<T as Table>::Value>, DatabaseError>;

// Sealed trait helper to prevent misuse of the Database API.
mod sealed {
    use crate::{database::Database, mock::DatabaseMock, DatabaseEnv};
    use std::sync::Arc;

    /// Sealed trait to limit the implementors of the Database trait.
    pub trait Sealed: Sized {}

    impl<DB: Database> Sealed for &DB {}
    impl<DB: Database> Sealed for Arc<DB> {}
    impl Sealed for DatabaseEnv {}
    impl Sealed for DatabaseMock {}

    #[cfg(any(test, feature = "test-utils"))]
    impl<DB: Database> Sealed for crate::test_utils::TempDatabase<DB> {}
}
pub(crate) use sealed::Sealed;
